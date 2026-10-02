use super::checks::{diff_checks, run_commands, CheckResult, Status};
use super::reviewer::{review, Finding, CHECKLISTS};
use crate::fingerprint::{Fingerprint, VerifyKind};
use crate::llm::client::LlmClient;
use crate::session::checkpoint::Checkpoints;
use std::future::Future;
use std::sync::Arc;
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
    /// warm language servers shared across rounds and tasks (None: start and stop them per round)
    pub lsp: Option<LspHandle>,
}

#[derive(Clone)]
pub struct LspHandle {
    pub pool: Arc<super::lsp::LspPool>,
    pub specs: Arc<Vec<super::lsp::LspSpec>>,
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
    // Everything that can overlap does: build/test commands run while the diff is inspected, and once the diff is known
    // the formatter, the language servers and the LLM reviewer run alongside the commands.
    let cmds = run_commands(ctx.cwd, &ctx.fp.verify, ctx.cancel.clone(), Duration::from_secs(600));
    let rest = async {
        let dc = diff_checks(ctx.cp, ctx.base, ctx.allowed_globs.as_deref()).await;
        let touched: Vec<String> = dc.changed.iter().filter(|f| !dc.deleted.contains(f)).cloned().collect();
        let fmt = super::format::format_check(ctx.cwd, ctx.cp, ctx.base, &touched);
        let lsp = async {
            let (pool, specs, owned) = match &ctx.lsp {
                Some(h) => (h.pool.clone(), h.specs.clone(), false),
                None => (super::lsp::LspPool::new(), Arc::new(super::lsp::load_specs(ctx.cwd, &crate::config::process_env())), true),
            };
            if specs.is_empty() {
                return None;
            }
            let r = super::lsp::diagnostics_check_pooled(&pool, ctx.cwd, ctx.cp, ctx.base, &touched, &specs).await;
            if owned {
                pool.shutdown().await;
            }
            r
        };
        let review_fut = async {
            let (mut findings, mut dropped, mut ran, mut stats) = (Vec::new(), 0, false, (0usize, 0usize, 0usize));
            if let Some(llm) = ctx.llm {
                if !dc.diff.trim().is_empty() && !dc.changed.is_empty() {
                    let checklist = CHECKLISTS[(round - 1).min(CHECKLISTS.len() - 1)];
                    // a reviewer failure never blocks: deterministic checks decide
                    if let Ok(r) = review(llm, ctx.cwd, &dc.diff, &dc.changed, ctx.acceptance, checklist, ctx.cancel.clone()).await {
                        stats = (r.valid.len() + r.dropped, r.valid.len(), r.dropped);
                        findings = r.valid;
                        dropped = r.dropped;
                        ran = !r.malformed;
                    }
                }
            }
            (findings, dropped, ran, stats)
        };
        let (fmt, lsp, reviewed) = tokio::join!(fmt, lsp, review_fut);
        (dc, fmt, lsp, reviewed)
    };
    let (cmd_res, (dc, fmt, lsp, (findings, dropped, ran, rstats))) = tokio::join!(cmds, rest);
    let mut checks: Vec<CheckResult> = cmd_res.into_iter().chain(dc.results).collect();
    checks.extend(lsp);
    checks.extend(fmt);
    st.stats.raised += rstats.0;
    st.stats.valid += rstats.1;
    st.stats.dropped += rstats.2;
    st.changed = dc.changed;
    let blocking: Vec<Finding> = findings.iter().filter(|f| is_blocking(f, round, ctx.max_rounds)).cloned().collect();
    st.stats.blockers += blocking.len();
    let det_fail = checks.iter().any(|c| c.status == Status::Fail);
    // a pass needs a build/test/type command that actually ran and passed: a command that found nothing to run
    // (skipped) verified nothing, and neither do language-server diagnostics alone
    let verified = checks.iter().any(|c| c.status == Status::Pass && ctx.fp.verify.iter().any(|v| v.name == c.name && matches!(v.kind, VerifyKind::Test | VerifyKind::Build | VerifyKind::Types)));
    let verdict = if det_fail || !blocking.is_empty() { Verdict::Fail } else if has_runnable && verified { Verdict::Pass } else { Verdict::Unverified };
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
