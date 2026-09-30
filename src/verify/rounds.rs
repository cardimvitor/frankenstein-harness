use super::checks::{diff_checks, run_commands, CheckResult, Status};
use super::reviewer::{review, Finding, CHECKLISTS};
use crate::fingerprint::{Fingerprint, VerifyKind};
use crate::llm::client::LlmClient;
use crate::session::checkpoint::Checkpoints;
use std::future::Future;
use std::path::Path;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Unverified,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Unverified => "unverified",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RoundReport {
    pub round: usize,
    pub checks: Vec<CheckResult>,
    pub findings: Vec<Finding>,
    pub dropped_findings: usize,
    pub reviewer_ran: bool,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, Default)]
pub struct ReviewerStats {
    pub raised: usize,
    pub valid: usize,
    pub dropped: usize,
    pub blockers: usize,
}

#[derive(Clone, Debug)]
pub struct VerifyReport {
    pub verdict: Verdict,
    pub rounds: Vec<RoundReport>,
    pub reason: String,
    pub changed: Vec<String>,
    pub reviewer: ReviewerStats,
}

pub struct VerifyCtx<'a> {
    pub cwd: &'a Path,
    pub cp: &'a Checkpoints,
    pub base: &'a str,
    pub fp: &'a Fingerprint,
    pub llm: Option<&'a LlmClient>,
    pub acceptance: &'a [String],
    pub allowed_globs: Option<Vec<String>>,
    pub max_rounds: usize,
    pub cancel: Option<CancellationToken>,
}

#[derive(Default)]
pub struct VerifyState {
    pub rounds: Vec<RoundReport>,
    pub stats: ReviewerStats,
    pub changed: Vec<String>,
}

fn is_blocking(f: &Finding, round: usize, max_rounds: usize) -> bool {
    f.severity == "blocker" || (f.severity == "major" && max_rounds > 2 && round > 1)
}

pub fn feedback_from(r: &RoundReport) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in &r.checks {
        if c.status == Status::Fail {
            parts.push(format!("CHECK FAILED: {}\n{}", c.name, c.detail));
        }
    }
    for f in &r.findings {
        parts.push(format!("REVIEW ({}) {}:{}: {}", f.severity, f.file, f.line, f.claim));
    }
    format!("Verification round {} did not pass. Fix these problems (only what is listed), then stop:\n\n{}", r.round, parts.join("\n\n"))
}

/// One verification round: parallel deterministic checks and an LLM review; deterministic results decide.
pub async fn run_round(ctx: &VerifyCtx<'_>, st: &mut VerifyState, round: usize) -> RoundReport {
    let has_runnable = ctx.fp.verify.iter().any(|c| matches!(c.kind, VerifyKind::Test | VerifyKind::Build | VerifyKind::Types));
    let (cmd_res, dc) = tokio::join!(
        run_commands(ctx.cwd, &ctx.fp.verify, ctx.cancel.clone(), Duration::from_secs(600)),
        diff_checks(ctx.cp, ctx.base, ctx.allowed_globs.as_deref())
    );
    let checks: Vec<CheckResult> = cmd_res.into_iter().chain(dc.results).collect();
    let (mut findings, mut dropped, mut ran) = (Vec::new(), 0, false);
    if let Some(llm) = ctx.llm {
        if !dc.diff.trim().is_empty() && !dc.changed.is_empty() {
            let checklist = CHECKLISTS[(round - 1).min(CHECKLISTS.len() - 1)];
            // a reviewer failure never blocks: deterministic checks decide
            if let Ok(r) = review(llm, ctx.cwd, &dc.diff, &dc.changed, ctx.acceptance, checklist, ctx.cancel.clone()).await {
                st.stats.raised += r.valid.len() + r.dropped;
                st.stats.valid += r.valid.len();
                st.stats.dropped += r.dropped;
                findings = r.valid;
                dropped = r.dropped;
                ran = !r.malformed;
            }
        }
    }
    st.changed = dc.changed;
    let blocking: Vec<Finding> = findings.iter().filter(|f| is_blocking(f, round, ctx.max_rounds)).cloned().collect();
    st.stats.blockers += blocking.len();
    let det_fail = checks.iter().any(|c| c.status == Status::Fail);
    let verdict = if det_fail || !blocking.is_empty() { Verdict::Fail } else if has_runnable { Verdict::Pass } else { Verdict::Unverified };
    let shown = if blocking.is_empty() { findings.into_iter().filter(|f| f.severity != "minor").collect() } else { blocking };
    let report = RoundReport { round, checks, findings: shown, dropped_findings: dropped, reviewer_ran: ran, verdict };
    st.rounds.push(report.clone());
    report
}

pub fn finalize(st: VerifyState) -> VerifyReport {
    let last = st.rounds.last().cloned();
    match last {
        Some(l) if l.verdict != Verdict::Fail => VerifyReport {
            verdict: l.verdict,
            reason: if l.verdict == Verdict::Pass { format!("passed in round {}", l.round) } else { "no build/test command available: result is unverified".into() },
            changed: st.changed,
            reviewer: st.stats,
            rounds: st.rounds,
        },
        Some(l) => {
            let mut failing: Vec<String> = l.checks.iter().filter(|c| c.status == Status::Fail).map(|c| c.name.clone()).collect();
            failing.extend(l.findings.iter().map(|f| format!("{}:{}", f.file, f.line)));
            VerifyReport { verdict: Verdict::Fail, reason: format!("still failing after {} round(s): {}", st.rounds.len(), failing.join(", ")), changed: vec![], reviewer: st.stats, rounds: st.rounds }
        }
        None => VerifyReport { verdict: Verdict::Fail, reason: "aborted".into(), changed: vec![], reviewer: st.stats, rounds: vec![] },
    }
}

/// Convenience loop for callers whose fixer does not need to borrow their own state.
pub async fn verify_loop<F, Fut>(ctx: &VerifyCtx<'_>, mut fix: F) -> VerifyReport
where
    F: FnMut(String, usize) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut st = VerifyState::default();
    for round in 1..=ctx.max_rounds {
        if ctx.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
            break;
        }
        let r = run_round(ctx, &mut st, round).await;
        if r.verdict != Verdict::Fail {
            break;
        }
        if round < ctx.max_rounds {
            fix(feedback_from(&r), round).await;
        }
    }
    finalize(st)
}
