//! Max-parallel mode: parallel sub-agents, then ONE agent that consolidates all of their findings and solutions.
//!
//! Two situations, one consolidator:
//! - the plan splits into file-disjoint parts: workers implement them in parallel (see `master`), then the consolidator
//!   reviews the combined result and reconciles it;
//! - the work does not split (the usual case for a single issue): read-only scouts investigate different angles in
//!   parallel, each returning findings and a proposed solution, and the main agent consolidates them and implements
//!   one coherent change.
//! Scouts cannot edit (plan mode); a scout that fails or runs out of budget is simply left out.

use super::governor::Governor;
use super::master::WorkerResult;
use crate::agent::runner::{run_agent, AgentOptions, Events};
use crate::types::Mode;
use std::sync::Arc;

pub const SCOUT_FOCI: [(&str, &str); 4] = [
    ("Locate", "Find the code the task concerns: the files, functions and data flow involved, and the root cause or the exact place to extend. Give paths and line numbers."),
    ("Tests", "Find the existing tests, fixtures, test commands and conventions that apply, how the behaviour will be verified, and which tests must be added or updated."),
    ("Impact", "Find the callers, public APIs, configuration and docs the change touches, the compatibility risks and the edge cases that must not break."),
    ("Solution", "Propose a concrete, minimal change: which files and edits, one or two alternatives, and the trade-offs. Do not write the full code."),
];

#[derive(Clone, Debug)]
pub struct Finding {
    pub focus: String,
    pub text: String,
    pub tokens: u64,
}

fn scout_message(goal: &str, n: usize, focus: (&str, &str)) -> String {
    format!(
        "You are one of {n} parallel read-only scouts investigating a coding task for a team. You cannot edit files. Your angle: {title} - {what}\n\nTask:\n{goal}\n\nRules: stay on your angle; be concrete (file paths, function and test names, line numbers, exact commands); do not write the whole solution; finish within about 10 tool calls. End with a report of at most 350 words under the headings \"Findings\" and \"Proposed solution\" (write \"none\" if you have no proposal).",
        title = focus.0,
        what = focus.1
    )
}

/// Runs up to `n` scouts (at most one per focus) in parallel under the governor and returns the reports that finished.
pub async fn run_scouts(base: &AgentOptions, goal: &str, n: usize, gov: &Arc<Governor>, budget_each: Option<u64>) -> Vec<Finding> {
    let n = n.min(SCOUT_FOCI.len());
    let mut handles = Vec::new();
    for (i, focus) in SCOUT_FOCI.iter().take(n).enumerate() {
        let mut ao = base.clone();
        ao.mode = Mode::Plan;
        ao.max_steps = 12;
        ao.token_budget = budget_each;
        ao.history = None;
        // scouts must not overwrite the session history or flood the UI with their steps
        ao.events = Events { notice: ao.events.notice.clone(), ..Default::default() };
        let msg = scout_message(goal, n, *focus);
        let gov = gov.clone();
        let name = format!("{} (scout {})", focus.0, i + 1);
        handles.push(tokio::spawn(async move {
            let r = gov.run(run_agent(&msg, ao)).await;
            (name, r)
        }));
    }
    let mut out = Vec::new();
    for h in handles {
        if let Ok((name, r)) = h.await {
            let text = r.final_text.trim().to_string();
            if !text.is_empty() && r.stopped != crate::agent::runner::Stopped::Error {
                out.push(Finding { focus: name, text, tokens: r.tokens });
            }
        }
    }
    out
}

/// The scouts' reports, ready to append to the consolidating agent's task.
pub fn render_findings(findings: &[Finding]) -> String {
    let mut s = String::from("Parallel read-only scouts already investigated this task. Their reports follow (they can be wrong: verify what you rely on).\n");
    for f in findings {
        s.push_str(&format!("\n### {}\n{}\n", f.focus, f.text));
    }
    s.push_str("\nYou are the single agent that consolidates all of this: choose the best-supported root cause and solution, resolve disagreements between the scouts, implement ONE coherent change, run the project's checks and finish.");
    s
}

/// The task for the consolidating agent after file-partitioned workers finished.
pub fn consolidator_message(task_text: &str, workers: &[WorkerResult]) -> String {
    let mut s = format!("{task_text}\n\n{} workers implemented parts of this task in parallel, each restricted to its own files. You are the consolidating agent. Review the combined result (git status, git diff), reconcile anything the parts left inconsistent (shared types, imports, names, API shapes), make the edits the workers asked for in files they did not own, complete whatever is missing, run the project's checks, fix what fails, and finish.\n\nWorker reports:", workers.len());
    for w in workers {
        s.push_str(&format!("\n- {} (touched: {}): {}", w.id, if w.touched.is_empty() { "nothing".to_string() } else { w.touched.join(", ") }, w.summary.trim()));
        for b in &w.blocked {
            s.push_str(&format!("\n  needed an edit in {}: {}", b.path, b.reason));
        }
    }
    s
}
