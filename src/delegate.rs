//! `fh delegate "<task>"`: the large model plans, a small worker model writes the code once, the large model verifies.
//!
//! This is the "27B + 9B" experiment: does a cheap worker help? The flow is fixed and one-way:
//!   1. planner (the main model, `FH_ENDPOINT`/`FH_MODEL`) reads a repository snapshot and writes a work order;
//!   2. worker (`FH_WORKER_ENDPOINT`/`FH_WORKER_MODEL`) gets the snapshot, the task and the work order and answers with
//!      SEARCH/REPLACE blocks (the same format as `fh direct`), applied as written;
//!   3. the planner then runs the normal fh loop on the result (tools, checks, fixes) with the worker's attempt as the
//!      starting state. The worker is never called a second time, whatever the verification finds.
//! Steps 1 and 2 live here; step 3 is the engine run that `cli.rs` starts with [`verification_task`].

use crate::config::{Config, Env};
use crate::direct::{apply_edits, build_context, parse_edits, SYSTEM as WORKER_SYSTEM};
use crate::llm::client::{ChatOptions, LlmClient, LlmStats};
use crate::types::{Message, Thinking};
use serde_json::{json, Value};
use std::path::Path;

const PLANNER_SYSTEM: &str = "You are the lead engineer. A smaller worker model will write the code from your work order, once, and you will review it afterwards. You are given a snapshot of a repository and a task. Write a precise work order: the root cause or the design, the exact files and functions to change, the behavior required (including edge cases and any tests or interfaces the task names), and the mistakes to avoid. Do not write the full patch and do not write SEARCH/REPLACE blocks. Be concrete and short.";

pub struct Prepared {
    pub plan: String,
    pub worker_reply: String,
    pub changed: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub edits: usize,
    pub planner: LlmStats,
    pub worker: LlmStats,
    pub error: Option<String>,
}

/// The worker's config: same settings as the main model, other endpoint and model name.
pub fn worker_config(cfg: &Config, env: &Env) -> Result<Config, String> {
    let endpoint = env.get("FH_WORKER_ENDPOINT").filter(|s| !s.is_empty()).ok_or("fh delegate needs FH_WORKER_ENDPOINT (the worker model's OpenAI-compatible URL)")?;
    let model = env.get("FH_WORKER_MODEL").filter(|s| !s.is_empty()).ok_or("fh delegate needs FH_WORKER_MODEL (the worker model's served name)")?;
    let mut w = cfg.clone();
    w.endpoint = endpoint.trim_end_matches('/').to_string();
    w.model = model.clone();
    if let Some(x) = env.get("FH_WORKER_CONTEXT_WINDOW").and_then(|s| s.parse().ok()) {
        w.context_window = x;
    }
    w.metrics_url = format!("{}/metrics", crate::config::origin_of(&w.endpoint));
    Ok(w)
}

/// Steps 1 and 2: the work order, then the worker's one attempt applied to the workspace.
pub async fn prepare(cfg: &Config, wcfg: &Config, env: &Env, cwd: &Path, task: &str) -> Prepared {
    let mut out = Prepared { plan: String::new(), worker_reply: String::new(), changed: vec![], failed: vec![], edits: 0, planner: LlmStats::default(), worker: LlmStats::default(), error: None };
    let planner = LlmClient::new(cfg.clone(), env.clone());
    let budget = |c: &Config| (c.context_window as f64 * 3.0 * 0.6) as usize;
    let (snapshot, _, _) = build_context(cwd, task, budget(cfg).min(budget(wcfg)));
    let user = format!("{snapshot}\n\nTask:\n{task}");
    let res = planner.chat(ChatOptions { messages: vec![Message::system(PLANNER_SYSTEM), Message::user(user.clone())], thinking: Thinking::High, max_tokens: Some(8192), ..Default::default() }).await;
    out.planner = planner.stats();
    match res {
        Ok(r) => out.plan = r.content,
        Err(e) => {
            out.error = Some(format!("planner: {e}"));
            return out;
        }
    }
    let worker = LlmClient::new(wcfg.clone(), env.clone());
    let wuser = format!("{snapshot}\n\nTask:\n{task}\n\nWork order from the lead engineer (follow it):\n{}", out.plan);
    let max_tokens = (wcfg.context_window as u32 / 4).clamp(4096, 32768);
    let res = worker.chat(ChatOptions { messages: vec![Message::system(WORKER_SYSTEM), Message::user(wuser)], thinking: Thinking::High, max_tokens: Some(max_tokens), ..Default::default() }).await;
    out.worker = worker.stats();
    match res {
        Ok(r) => out.worker_reply = r.content,
        Err(e) => {
            out.error = Some(format!("worker: {e}"));
            return out;
        }
    }
    let edits = parse_edits(&out.worker_reply);
    out.edits = edits.len();
    let (changed, failed) = apply_edits(cwd, &edits);
    out.changed = changed;
    out.failed = failed;
    out
}

/// Step 3's prompt: the main model reviews and finishes the worker's attempt on its own.
pub fn verification_task(task: &str, p: &Prepared) -> String {
    let state = if p.changed.is_empty() {
        "The worker produced no applicable edits, so the workspace is unchanged.".to_string()
    } else {
        format!("The worker edited: {}.", p.changed.join(", "))
    };
    let failed = if p.failed.is_empty() { String::new() } else { format!(" These worker edits could not be applied: {}.", p.failed.iter().map(|(f, w)| format!("{f} ({w})")).collect::<Vec<_>>().join("; ")) };
    format!("{task}\n\n[Context] A smaller worker model already made a first attempt at this task from a work order. {state}{failed} The worker will not be called again. Review the current state of the workspace, run the project's checks, and fix or finish anything that is wrong or missing yourself; the task is only done when it is correct.")
}

pub fn stats_json(s: &LlmStats) -> Value {
    json!({"requests": s.requests, "promptTokens": s.prompt_tokens, "completionTokens": s.completion_tokens, "cachedTokens": s.cached_tokens})
}

pub fn report_json(p: &Prepared) -> Value {
    json!({
        "edits": p.edits, "changed": p.changed, "failed": p.failed.iter().map(|(f, w)| json!({"path": f, "why": w})).collect::<Vec<_>>(),
        "error": p.error, "planner": stats_json(&p.planner), "worker": stats_json(&p.worker), "plan": p.plan.chars().take(3000).collect::<String>(),
    })
}
