use super::governor::Governor;
use super::partition::plan_waves;
use crate::agent::{run_agent, AgentOptions, Stopped};
use crate::funnel::intake::Subtask;
use crate::llm::client::LlmClient;
use crate::types::Mode;
use crate::util::paths::matches_any;
use crate::util::proc::ShellWrap;
use futures_util::future::join_all;
use crate::tools::{Tool, ToolCtx, ToolRef, ToolResult};
use crate::types::ToolSpec;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct WorkerResult {
    pub id: String,
    pub summary: String,
    pub touched: Vec<String>,
    pub stopped: Stopped,
    pub steps: usize,
    pub tokens: u64,
    /// edits this worker needed in files it does not own
    pub blocked: Vec<BlockedRequest>,
}

#[derive(Clone, Debug)]
pub struct BlockedRequest {
    pub worker: String,
    pub path: String,
    pub reason: String,
}

/// Worker-only tool: ask the master for an edit in a file owned by someone else, then keep going.
pub struct RequestEdit {
    spec: ToolSpec,
    worker: String,
    log: Arc<Mutex<Vec<BlockedRequest>>>,
}

impl RequestEdit {
    pub fn new(worker: &str, log: Arc<Mutex<Vec<BlockedRequest>>>) -> Self {
        RequestEdit {
            spec: ToolSpec {
                name: "request_edit".into(),
                description: "You are not allowed to edit a file (owned by another worker or outside your scope) but your subtask needs a change there. Describe the change; the master routes it to the owner. Then continue with the rest of your subtask.".into(),
                parameters: json!({"type":"object","properties":{"path":{"type":"string"},"reason":{"type":"string","description":"exactly what must change and why"}},"required":["path","reason"]}),
            },
            worker: worker.to_string(),
            log,
        }
    }
}

#[async_trait]
impl Tool for RequestEdit {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    fn read_only(&self) -> bool {
        true
    }
    async fn execute(&self, args: &Value, _ctx: &ToolCtx) -> ToolResult {
        let (Some(path), Some(reason)) = (args["path"].as_str(), args["reason"].as_str()) else { return ToolResult::err("path and reason are required") };
        self.log.lock().unwrap().push(BlockedRequest { worker: self.worker.clone(), path: path.trim_start_matches("./").to_string(), reason: reason.to_string() });
        ToolResult::ok("recorded; the master will have the owner make this change. Continue with the rest of your subtask.")
    }
}

pub struct MasterOptions {
    pub llm: LlmClient,
    pub cwd: PathBuf,
    pub mode: Mode,
    pub context: String,
    pub governor: Arc<Governor>,
    pub cancel: Option<CancellationToken>,
    pub context_window: usize,
    pub max_steps: usize,
    pub notice: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    pub wrap_shell: Option<ShellWrap>,
    pub confirm: Option<crate::agent::ConfirmFn>,
    pub extra_tools: Vec<crate::tools::ToolRef>,
    /// total task token budget (0 = unlimited); 60% is split evenly across the workers
    pub task_budget: u64,
    pub llm_compaction: bool,
}

fn worker_task(s: &Subtask, all: &[Subtask], goal: &str) -> String {
    let others: Vec<String> = all.iter().filter(|x| x.id != s.id).map(|x| format!("{}: {}", x.id, x.goal)).collect();
    [
        format!("You are a worker on a larger task. Overall goal: {goal}"),
        format!("Your subtask ({}): {}", s.id, s.goal),
        format!("You may only modify files matching: {}. Other files are owned by other workers; read them if needed but do not edit them.", s.files.join(", ")),
        format!("Other subtasks running or done: {}", if others.is_empty() { "none".to_string() } else { others.join(" | ") }),
        "If your subtask needs a change in a file you do not own, call request_edit with the path and the exact change, then continue; do not try to work around the ownership rule.".to_string(),
        "Do only your subtask. Finish with a 2-3 line summary of what you changed.".to_string(),
    ]
    .join("\n")
}

async fn run_worker(o: &MasterOptions, s: &Subtask, all: &[Subtask], goal: &str, extra_note: &str, budget: Option<u64>) -> WorkerResult {
    let blocked: Arc<Mutex<Vec<BlockedRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let mut ao = AgentOptions::new(o.llm.clone(), o.cwd.clone(), o.mode);
    ao.max_steps = o.max_steps;
    ao.cancel = o.cancel.clone();
    ao.owned_globs = Some(s.files.clone());
    ao.context = Some(o.context.clone());
    ao.context_window = o.context_window;
    ao.wrap_shell = o.wrap_shell.clone();
    ao.confirm = o.confirm.clone();
    ao.tools.extend(o.extra_tools.iter().cloned());
    ao.tools.push(Arc::new(RequestEdit::new(&s.id, blocked.clone())) as ToolRef);
    ao.token_budget = budget;
    ao.llm_compaction = o.llm_compaction;
    let mut task = worker_task(s, all, goal);
    if !extra_note.is_empty() {
        task.push_str("\n\n");
        task.push_str(extra_note);
    }
    let r = run_agent(&task, ao).await;
    let b = blocked.lock().unwrap().clone();
    WorkerResult { id: s.id.clone(), summary: if r.final_text.is_empty() { r.error.clone().unwrap_or_default() } else { r.final_text }, touched: r.touched, stopped: r.stopped, steps: r.steps, tokens: r.tokens, blocked: b }
}

/// Master: dispatch narrow-scoped workers wave by wave under the governor and collect their summaries.
/// Requests for edits outside a worker's ownership are routed afterwards: to the owning worker, or to an extra
/// worker when nobody owns the file. Nothing from a worker is shown to the user; only the master's result is.
pub async fn run_workers(goal: &str, subtasks: &[Subtask], o: &MasterOptions) -> Vec<WorkerResult> {
    let waves = plan_waves(subtasks);
    let mut results: Vec<WorkerResult> = Vec::new();
    // 60% of the task budget is shared by the workers; the rest stays for verification rounds and fixes
    let budget = if o.task_budget > 0 { Some((o.task_budget * 6 / 10 / subtasks.len().max(1) as u64).max(20_000)) } else { None };
    o.governor.start(Duration::from_secs(2));
    for (wi, wave) in waves.iter().enumerate() {
        if o.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
            break;
        }
        if let Some(n) = &o.notice {
            n(&format!("wave {}/{}: {} worker(s)", wi + 1, waves.len(), wave.len()));
        }
        let rs = join_all(wave.iter().map(|s| {
            let gov = o.governor.clone();
            async move { gov.run(run_worker(o, s, subtasks, goal, "", budget)).await }
        }))
        .await;
        results.extend(rs);
    }
    // route blocked requests: owner worker first, an extra worker for files nobody owns (one round only)
    let requests: Vec<BlockedRequest> = results.iter().flat_map(|r| r.blocked.clone()).collect();
    if !requests.is_empty() && !o.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
        let mut by_owner: BTreeMap<String, (Subtask, Vec<BlockedRequest>)> = BTreeMap::new();
        for r in &requests {
            let (key, sub) = match subtasks.iter().find(|t| matches_any(&r.path, &t.files) && t.id != r.worker) {
                Some(t) => (t.id.clone(), t.clone()),
                None => (format!("extra-{}", r.path.rsplit_once('/').map(|(d, _)| d).unwrap_or(".")), Subtask { id: format!("extra-{}", by_owner.len() + 1), goal: "Make the changes other workers asked for in files nobody owned".into(), files: vec![r.path.clone()], deps: vec![] }),
            };
            let e = by_owner.entry(key).or_insert((sub, vec![]));
            if !e.0.files.contains(&r.path) && e.0.id.starts_with("extra-") {
                e.0.files.push(r.path.clone());
            }
            e.1.push(r.clone());
        }
        if let Some(n) = &o.notice {
            n(&format!("routing {} edit request(s) to {} worker(s)", requests.len(), by_owner.len()));
        }
        let rs = join_all(by_owner.into_values().map(|(sub, reqs)| {
            let gov = o.governor.clone();
            async move {
                let note = format!("Requests from other workers for files you own:\n{}", reqs.iter().map(|r| format!("- {} (from {}): {}", r.path, r.worker, r.reason)).collect::<Vec<_>>().join("\n"));
                let mut w = gov.run(run_worker(o, &sub, subtasks, goal, &note, budget)).await;
                w.id = format!("{} (follow-up)", w.id);
                w
            }
        }))
        .await;
        results.extend(rs);
    }
    o.governor.stop();
    results
}

/// Which subtask (worker) owns the files named in a failure report; unmatched failures go to the master's own fixer.
pub fn owners_of(subtasks: &[Subtask], files: &[String]) -> (BTreeMap<String, Vec<String>>, Vec<String>) {
    let mut owned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unowned = Vec::new();
    for f in files {
        match subtasks.iter().find(|t| matches_any(f, &t.files)) {
            Some(s) => owned.entry(s.id.clone()).or_default().push(f.clone()),
            None => unowned.push(f.clone()),
        }
    }
    (owned, unowned)
}
