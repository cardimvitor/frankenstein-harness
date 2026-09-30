use super::context::compact;
use super::permissions::{decide, Decision};
use super::prompt::SYSTEM_PROMPT;
use crate::config::{redact, Env};
use crate::llm::client::{Callback, ChatOptions, LlmClient};
use crate::tools::{all_tools, ToolCtx, ToolRef, ToolResult};
use crate::types::*;
use crate::util::proc::ShellWrap;
use futures_util::future::join_all;
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub type ConfirmFn = Arc<dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

#[derive(Clone, Default)]
pub struct Events {
    pub text: Option<Callback>,
    pub reasoning: Option<Callback>,
    pub tool_start: Option<Arc<dyn Fn(&ToolCall) + Send + Sync>>,
    pub tool_end: Option<Arc<dyn Fn(&ToolCall, &ToolResult, u64) + Send + Sync>>,
    pub notice: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    /// called with the full message history after every step (session persistence)
    pub on_messages: Option<Arc<dyn Fn(&[Message]) + Send + Sync>>,
}

#[derive(Clone)]
pub struct AgentOptions {
    pub llm: LlmClient,
    pub cwd: PathBuf,
    pub mode: Mode,
    /// None = automatic: thinking on for the first step and after repeated tool failures, off otherwise
    pub thinking: Option<Thinking>,
    pub max_steps: usize,
    pub cancel: Option<CancellationToken>,
    pub tools: Vec<ToolRef>,
    pub owned_globs: Option<Vec<String>>,
    pub confirm: Option<ConfirmFn>,
    pub events: Events,
    /// Task-specific context inserted as the second system message (after the stable prefix).
    pub context: Option<String>,
    pub history: Option<Vec<Message>>,
    pub context_window: usize,
    pub wrap_shell: Option<ShellWrap>,
    pub hooks: Option<Arc<crate::hooks::Hooks>>,
}

impl AgentOptions {
    pub fn new(llm: LlmClient, cwd: impl Into<PathBuf>, mode: Mode) -> Self {
        AgentOptions { llm, cwd: cwd.into(), mode, thinking: None, max_steps: 40, cancel: None, tools: all_tools(), owned_globs: None, confirm: None, events: Events::default(), context: None, history: None, context_window: 131072, wrap_shell: None, hooks: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    Done,
    MaxSteps,
    Loop,
    Aborted,
    Error,
}

impl Stopped {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stopped::Done => "done",
            Stopped::MaxSteps => "max_steps",
            Stopped::Loop => "loop",
            Stopped::Aborted => "aborted",
            Stopped::Error => "error",
        }
    }
}

pub struct AgentResult {
    pub final_text: String,
    pub messages: Vec<Message>,
    pub steps: usize,
    pub touched: Vec<String>,
    pub stopped: Stopped,
    pub error: Option<String>,
    pub tool_calls: usize,
    pub failed_tools: usize,
}

pub async fn run_agent(task: &str, o: AgentOptions) -> AgentResult {
    let env: Env = o.llm.env.clone();
    let by_name: std::collections::HashMap<String, ToolRef> = o.tools.iter().map(|t| (t.spec().name.clone(), t.clone())).collect();
    let specs: Vec<ToolSpec> = o.tools.iter().map(|t| t.spec().clone()).collect();
    let mut ctx = ToolCtx::new(o.cwd.clone());
    ctx.cancel = o.cancel.clone();
    ctx.owned_globs = o.owned_globs.clone();
    ctx.wrap_shell = o.wrap_shell.clone();
    let ctx = Arc::new(ctx);

    let mut messages: Vec<Message> = match &o.history {
        Some(h) => h.clone(),
        None => {
            let mut v = vec![Message::system(SYSTEM_PROMPT)];
            if let Some(c) = &o.context {
                v.push(Message::system(c.clone()));
            }
            v
        }
    };
    messages.push(Message::user(task));
    let mut recent: Vec<String> = Vec::new();
    let (mut tool_calls, mut failed_tools, mut consecutive_fail) = (0usize, 0usize, 0usize);
    let mut final_text = String::new();
    let mut stopped = Stopped::MaxSteps;
    let mut error: Option<String> = None;
    let budget = (o.context_window as f64 * 0.72) as usize;

    let mut step = 0;
    while step < o.max_steps {
        if o.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
            stopped = Stopped::Aborted;
            break;
        }
        let level = match o.thinking {
            Some(t) => t,
            None => {
                if step == 0 || consecutive_fail >= 2 {
                    Thinking::High
                } else {
                    Thinking::Off
                }
            }
        };
        let (compacted, pruned) = compact(&messages, budget, 6);
        if pruned > 0 {
            messages = compacted;
            if let Some(n) = &o.events.notice {
                n(&format!("context compacted ({pruned} items)"));
            }
        }
        let r = match o
            .llm
            .chat(ChatOptions { messages: messages.clone(), tools: specs.clone(), thinking: level, cancel: o.cancel.clone(), on_content: o.events.text.clone(), on_reasoning: o.events.reasoning.clone(), ..Default::default() })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                if e.aborted || o.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
                    stopped = Stopped::Aborted;
                } else {
                    stopped = Stopped::Error;
                    error = Some(redact(&e.msg, &env));
                }
                break;
            }
        };
        let wire: Vec<WireToolCall> = r
            .tool_calls
            .iter()
            .map(|t| WireToolCall { id: t.id.clone(), kind: "function".into(), function: WireFunction { name: t.name.clone(), arguments: if t.parse == ParseState::Malformed { "{}".into() } else { t.args.to_string() } } })
            .collect();
        messages.push(Message::assistant(if r.content.is_empty() { None } else { Some(r.content.clone()) }, wire));
        step += 1;

        if r.tool_calls.is_empty() {
            if r.finish == "length" {
                messages.push(Message::user("Your reply was cut off. Continue briefly."));
                continue;
            }
            final_text = r.content;
            stopped = Stopped::Done;
            break;
        }

        // loop detection signatures, recorded in call order
        let reps: Vec<usize> = r
            .tool_calls
            .iter()
            .map(|c| {
                let sig = format!("{}{}", c.name, c.args);
                recent.push(sig.clone());
                if recent.len() > 12 {
                    recent.remove(0);
                }
                recent.iter().filter(|s| **s == sig).count()
            })
            .collect();

        let run_one = |call: ToolCall, rep: usize| {
            let by_name = &by_name;
            let ctx = ctx.clone();
            let o = &o;
            async move {
                let Some(tool) = by_name.get(&call.name) else {
                    let mut names: Vec<&String> = by_name.keys().collect();
                    names.sort();
                    return ToolResult::err(format!("unknown tool \"{}\". Available: {}", call.name, names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
                };
                if call.parse == ParseState::Malformed {
                    return ToolResult::err("tool arguments were not valid JSON; resend the call with valid JSON arguments");
                }
                match decide(o.mode, tool.as_ref(), &call.args) {
                    Decision::Allow => {}
                    Decision::Deny { reason, ask } => {
                        let approved = if ask { match &o.confirm { Some(c) => c(call.name.clone(), call.args.clone()).await, None => false } } else { false };
                        if !approved {
                            return ToolResult::err(if ask { "denied by user".to_string() } else { reason });
                        }
                    }
                }
                if let Some(h) = &o.hooks {
                    let out = h.fire(crate::hooks::HookEvent::PreToolUse, serde_json::json!({"tool_name": call.name, "tool_input": call.args})).await;
                    if let Some(why) = out.blocked {
                        return ToolResult::err(format!("blocked by hook: {why}"));
                    }
                }
                let t0 = Instant::now();
                if let Some(f) = &o.events.tool_start {
                    f(&call);
                }
                let mut res = tool.execute(&call.args, &ctx).await;
                if let Some(h) = &o.hooks {
                    let out = h.fire(crate::hooks::HookEvent::PostToolUse, serde_json::json!({"tool_name": call.name, "tool_input": call.args, "tool_response": {"ok": res.ok, "output": res.output.chars().take(4000).collect::<String>()}})).await;
                    if let Some(why) = out.blocked {
                        res.output.push_str(&format!("\n[hook feedback: {why}]"));
                    }
                }
                if rep >= 3 {
                    res.output.push_str("\n[You have repeated this exact call several times. Try a different approach.]");
                }
                if let Some(f) = &o.events.tool_end {
                    f(&call, &res, t0.elapsed().as_millis() as u64);
                }
                res
            }
        };

        let all_read = r.tool_calls.iter().all(|t| by_name.get(&t.name).map(|x| x.read_only()).unwrap_or(false));
        let results: Vec<ToolResult> = if all_read {
            join_all(r.tool_calls.iter().cloned().zip(reps.iter().copied()).map(|(c, rep)| run_one(c, rep))).await
        } else {
            let mut v = Vec::new();
            for (c, rep) in r.tool_calls.iter().cloned().zip(reps.iter().copied()) {
                v.push(run_one(c, rep).await);
            }
            v
        };
        for (call, res) in r.tool_calls.iter().zip(results.iter()) {
            tool_calls += 1;
            if !res.ok {
                failed_tools += 1;
                consecutive_fail += 1;
            } else {
                consecutive_fail = 0;
            }
            messages.push(Message::tool(call.id.clone(), res.output.clone()));
        }
        if let Some(f) = &o.events.on_messages {
            f(&messages);
        }
        if recent.iter().any(|s| recent.iter().filter(|x| *x == s).count() >= 5) {
            stopped = Stopped::Loop;
            error = Some("stopped: repeated identical tool calls".into());
            break;
        }
    }
    if step >= o.max_steps && stopped == Stopped::MaxSteps {
        error = Some(format!("reached step limit ({})", o.max_steps));
    }
    let touched: Vec<String> = ctx.touched.lock().unwrap().iter().cloned().collect();
    AgentResult { final_text, messages, steps: step, touched, stopped, error, tool_calls, failed_tools }
}
