use super::governor::Governor;
use super::partition::plan_waves;
use crate::agent::{run_agent, AgentOptions, Stopped};
use crate::funnel::intake::Subtask;
use crate::llm::client::LlmClient;
use crate::types::Mode;
use crate::util::paths::matches_any;
use crate::util::proc::ShellWrap;
use futures_util::future::join_all;
use std::collections::BTreeMap;
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
}

fn worker_task(s: &Subtask, all: &[Subtask], goal: &str) -> String {
    let others: Vec<String> = all.iter().filter(|x| x.id != s.id).map(|x| format!("{}: {}", x.id, x.goal)).collect();
    [
        format!("You are a worker on a larger task. Overall goal: {goal}"),
        format!("Your subtask ({}): {}", s.id, s.goal),
        format!("You may only modify files matching: {}. Other files are owned by other workers; read them if needed but do not edit them.", s.files.join(", ")),
        format!("Other subtasks running or done: {}", if others.is_empty() { "none".to_string() } else { others.join(" | ") }),
        "Do only your subtask. Finish with a 2-3 line summary of what you changed.".to_string(),
    ]
    .join("\n")
}

/// Master: dispatch narrow-scoped workers wave by wave under the governor and collect their summaries.
/// Nothing from a worker is shown to the user; only the master's consolidated result is.
pub async fn run_workers(goal: &str, subtasks: &[Subtask], o: &MasterOptions) -> Vec<WorkerResult> {
    let waves = plan_waves(subtasks);
    let mut results = Vec::new();
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
            async move {
                gov.run(async {
                    let mut ao = AgentOptions::new(o.llm.clone(), o.cwd.clone(), o.mode);
                    ao.max_steps = o.max_steps;
                    ao.cancel = o.cancel.clone();
                    ao.owned_globs = Some(s.files.clone());
                    ao.context = Some(o.context.clone());
                    ao.context_window = o.context_window;
                    ao.wrap_shell = o.wrap_shell.clone();
                    ao.confirm = o.confirm.clone();
                    let r = run_agent(&worker_task(s, subtasks, goal), ao).await;
                    WorkerResult { id: s.id.clone(), summary: if r.final_text.is_empty() { r.error.clone().unwrap_or_default() } else { r.final_text }, touched: r.touched, stopped: r.stopped, steps: r.steps }
                })
                .await
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
