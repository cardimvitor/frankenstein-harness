use super::secrets::find_secrets;
use crate::fingerprint::{VerifyCmd, VerifyKind};
use crate::session::checkpoint::Checkpoints;
use crate::util::paths::matches_any;
use crate::util::proc::{clip, run, RunOpts};
use futures_util::future::join_all;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Skipped,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::Skipped => "skipped",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckKind {
    Build,
    Lint,
    Test,
    Types,
    Diff,
    Secret,
    TestsIntegrity,
    Patch,
}

impl From<VerifyKind> for CheckKind {
    fn from(k: VerifyKind) -> Self {
        match k {
            VerifyKind::Build => CheckKind::Build,
            VerifyKind::Lint => CheckKind::Lint,
            VerifyKind::Test => CheckKind::Test,
            VerifyKind::Types => CheckKind::Types,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CheckResult {
    pub name: String,
    pub status: Status,
    /// deterministic checks always decide; details are fed back to the worker
    pub detail: String,
    pub ms: u64,
    pub kind: CheckKind,
}

static TEST_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(^|/)(tests?|__tests__|spec)(/|$)|(\.|_)(test|spec)\.[a-z]+$|Tests?\.cs$|(^|/)test_[^/]+\.py$|_test\.(go|py)$").unwrap());
static SKIP_ADDED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\b(it|test|describe)\.skip\b|\bxit\(|\bxdescribe\(|\bxtest\(|@pytest\.mark\.skip|@unittest\.skip|\[Ignore\]|\[Fact\(Skip|t\.Skip\(|\.only\()").unwrap());
static HUNK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)").unwrap());

pub fn is_test_file(p: &str) -> bool {
    TEST_FILE.is_match(p)
}

#[derive(Clone, Debug)]
pub struct AddedLine {
    pub file: String,
    pub line: usize,
    pub text: String,
}

pub fn parse_diff_added(diff: &str) -> Vec<AddedLine> {
    let mut out = Vec::new();
    let (mut file, mut ln) = (String::new(), 0usize);
    for l in diff.lines() {
        if let Some(f) = l.strip_prefix("+++ ") {
            file = f.strip_prefix("b/").unwrap_or(f).to_string();
            continue;
        }
        if let Some(h) = HUNK.captures(l) {
            ln = h[1].parse().unwrap_or(0);
            continue;
        }
        if l.starts_with('+') && !l.starts_with("+++") {
            out.push(AddedLine { file: file.clone(), line: ln, text: l[1..].to_string() });
            ln += 1;
        } else if !l.starts_with('-') && !l.starts_with('\\') && !file.is_empty() {
            ln += 1;
        }
    }
    out
}

pub async fn run_commands(cwd: &Path, cmds: &[VerifyCmd], cancel: Option<CancellationToken>, timeout: Duration) -> Vec<CheckResult> {
    let exec = |c: VerifyCmd| {
        let cancel = cancel.clone();
        async move {
            let r = run(&c.cmd, cwd, RunOpts { timeout: Some(timeout), cancel, ..Default::default() }).await;
            let ok = r.code == Some(0) && !r.timed_out;
            CheckResult {
                name: c.name.clone(),
                kind: c.kind.into(),
                status: if ok { Status::Pass } else { Status::Fail },
                ms: r.ms,
                detail: if ok { "ok".into() } else { clip(&format!("exit {}{}\n{}\n{}", r.code.map(|c| c.to_string()).unwrap_or_else(|| "none".into()), if r.timed_out { " (timeout)" } else { "" }, r.stdout, r.stderr), 6000) },
            }
        }
    };
    // build/types/lint in parallel; tests afterwards, skipped when the build is broken
    let (pre, tests): (Vec<VerifyCmd>, Vec<VerifyCmd>) = cmds.iter().cloned().partition(|c| c.kind != VerifyKind::Test);
    let first = join_all(pre.into_iter().map(&exec)).await;
    let build_failed = first.iter().any(|r| r.status == Status::Fail && matches!(r.kind, CheckKind::Build | CheckKind::Types));
    let mut second = Vec::new();
    if build_failed {
        for t in tests {
            second.push(CheckResult { name: t.name, kind: CheckKind::Test, status: Status::Skipped, detail: "skipped: build failed".into(), ms: 0 });
        }
    } else {
        second = join_all(tests.into_iter().map(&exec)).await;
    }
    first.into_iter().chain(second).collect()
}

pub struct DiffChecks {
    pub results: Vec<CheckResult>,
    pub diff: String,
    pub changed: Vec<String>,
}

pub async fn diff_checks(cp: &Checkpoints, base: &str, allowed_globs: Option<&[String]>) -> DiffChecks {
    let t0 = std::time::Instant::now();
    let ch = cp.changed_since(base).await;
    let diff = cp.diff_since(base).await;
    let changed = ch.all();
    let mut results = Vec::new();
    let mk = |name: &str, kind: CheckKind, bad: Vec<String>, ok_msg: String| CheckResult {
        name: name.into(),
        kind,
        status: if bad.is_empty() { Status::Pass } else { Status::Fail },
        detail: if bad.is_empty() { ok_msg } else { bad.join("\n") },
        ms: t0.elapsed().as_millis() as u64,
    };
    match allowed_globs {
        Some(g) if !g.is_empty() => {
            let outside: Vec<String> = changed.iter().filter(|f| !matches_any(f, g)).map(|f| format!("outside planned scope: {f}")).collect();
            results.push(mk("diff scope", CheckKind::Diff, outside, format!("{} file(s) within scope", changed.len())));
        }
        _ => results.push(CheckResult { name: "diff scope".into(), kind: CheckKind::Diff, status: Status::Skipped, detail: "no planned scope".into(), ms: 0 }),
    }
    let added = parse_diff_added(&diff);
    let secret_hits: Vec<String> = added.iter().flat_map(|a| find_secrets(&a.text).into_iter().map(move |s| format!("{}:{}: possible {}", a.file, a.line, s))).collect();
    results.push(mk("secret scan", CheckKind::Secret, secret_hits, "no secrets in added lines".into()));
    let mut bad: Vec<String> = ch.deleted.iter().filter(|f| is_test_file(f)).map(|f| format!("deleted test file: {f}")).collect();
    for a in &added {
        if is_test_file(&a.file) && SKIP_ADDED.is_match(&a.text) {
            bad.push(format!("{}:{}: skipped/focused test added", a.file, a.line));
        }
    }
    results.push(mk("test integrity", CheckKind::TestsIntegrity, bad, "no tests deleted or skipped".into()));
    let conflict: Vec<String> = added.iter().filter(|a| a.text.starts_with("<<<<<<< ") || a.text.starts_with(">>>>>>> ") || a.text == "=======").map(|a| format!("{}:{}: conflict marker", a.file, a.line)).collect();
    results.push(mk("patch sanity", CheckKind::Patch, conflict, "no conflict markers".into()));
    let patch = cp.patch_since(base).await;
    let applies = match cp.patch_applies(base, &patch).await {
        Ok(()) => vec![],
        Err(e) => vec![format!("patch does not apply cleanly to the base: {e}")],
    };
    results.push(mk("patch applies", CheckKind::Patch, applies, "patch applies cleanly to the base".into()));
    DiffChecks { results, diff, changed }
}
