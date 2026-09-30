use super::corpus::{builtin_tasks, EvalTask};
use crate::config::{Config, Env};
use crate::engine::{Engine, HeadlessIo, TaskOptions};
use crate::skills::store::SkillStore;
use crate::types::Mode;
use crate::util::proc::{run, run_simple, RunOpts};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct EvalRow {
    pub id: String,
    pub runner: String,
    pub rep: usize,
    pub solved: bool,
    pub seconds: f64,
    pub verdict: Option<String>,
    pub rounds: Option<usize>,
    pub tool_calls: Option<u64>,
    pub failed_tools: Option<usize>,
    pub repaired: Option<u64>,
    pub malformed: Option<u64>,
    pub think_leaks: Option<u64>,
    pub tokens_out: Option<u64>,
    pub tokens_in: Option<u64>,
    pub requests: Option<u64>,
    pub acceptance: Option<f64>,
    pub prefix_hit: Option<f64>,
    /// verdict pass while the oracle fails
    pub verifier_false_negative: bool,
    pub gate_ms: Option<f64>,
    pub reviewer: Option<(usize, usize, usize, usize)>,
    pub error: Option<String>,
}

impl EvalRow {
    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "runner": self.runner, "rep": self.rep, "solved": self.solved, "seconds": self.seconds, "verdict": self.verdict, "rounds": self.rounds,
               "toolCalls": self.tool_calls, "failedTools": self.failed_tools, "repaired": self.repaired, "malformed": self.malformed, "thinkLeaks": self.think_leaks,
               "tokensOut": self.tokens_out, "tokensIn": self.tokens_in, "requests": self.requests, "acceptance": self.acceptance, "prefixHit": self.prefix_hit,
               "verifierFalseNegative": self.verifier_false_negative, "gateMs": self.gate_ms,
               "reviewer": self.reviewer.map(|r| json!({"raised": r.0, "valid": r.1, "dropped": r.2, "blockers": r.3})), "error": self.error})
    }
}

pub fn load_tasks(dir: &Path) -> Vec<EvalTask> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let tj = e.path().join("task.json");
            if !e.path().is_dir() || !tj.exists() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&std::fs::read_to_string(&tj).unwrap_or_default()) else { continue };
            let (Some(prompt), Some(oracle)) = (v["prompt"].as_str(), v["oracle"].as_str()) else { continue };
            out.push(EvalTask {
                id: v["id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| e.file_name().to_string_lossy().to_string()),
                prompt: prompt.into(),
                oracle: oracle.into(),
                files: vec![],
                repo_dir: Some(e.path().join("repo")),
                timeout_s: v["timeoutS"].as_u64().unwrap_or(900),
                tags: v["tags"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default(),
                requires: None,
            });
        }
    }
    if out.is_empty() { builtin_tasks() } else { out }
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}

pub async fn materialize(t: &EvalTask) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fh-eval-{}-{:x}", t.id, rand::random::<u32>()));
    let _ = std::fs::create_dir_all(&d);
    if let Some(r) = &t.repo_dir {
        let _ = copy_dir(r, &d);
    } else {
        for (f, body) in &t.files {
            let p = d.join(f);
            if let Some(par) = p.parent() {
                let _ = std::fs::create_dir_all(par);
            }
            let _ = std::fs::write(p, body);
        }
    }
    run_simple("git init -q && git add -A && git -c user.name=eval -c user.email=eval@local commit -qm base", &d, 30_000).await;
    d
}

async fn oracle_ok(t: &EvalTask, cwd: &Path) -> bool {
    run_simple(&t.oracle, cwd, 120_000).await.code == Some(0)
}

pub async fn run_one(cfg: &Config, env: &Env, t: &EvalTask, runner: &str, rep: usize, qwen_cmd: &str) -> EvalRow {
    let cwd = materialize(t).await;
    let t0 = Instant::now();
    let mut row = EvalRow { id: t.id.clone(), runner: runner.into(), rep, ..Default::default() };
    if runner.starts_with("fh") {
        // "fh-single" = the same harness with worker fan-out disabled: isolates what orchestration adds on this model
        let mut cfg = cfg.clone();
        if runner == "fh-single" {
            cfg.max_concurrency = 1;
        }
        let cfg = &cfg;
        let engine = Engine::new(cfg.clone(), env.clone(), Arc::new(HeadlessIo { log: None }), cwd.clone(), SkillStore::in_memory());
        let r = engine.run_task(&t.prompt, TaskOptions { auto: true, approval: Mode::Yolo, no_mine: true, ..Default::default() }).await;
        row.verdict = Some(r.verdict.clone());
        row.rounds = Some(r.rounds);
        row.tool_calls = Some(r.llm.tool_calls);
        row.failed_tools = r.agent.as_ref().map(|a| a.failed_tools);
        row.repaired = Some(r.llm.repaired);
        row.malformed = Some(r.llm.malformed);
        row.think_leaks = Some(r.llm.think_leaks);
        row.tokens_out = Some(r.llm.completion_tokens);
        row.tokens_in = Some(r.llm.prompt_tokens);
        row.requests = Some(r.llm.requests);
        row.acceptance = r.metrics.acceptance_rate;
        row.prefix_hit = r.metrics.prefix_hit_rate;
        row.gate_ms = r.gate.as_ref().map(|g| g.0);
        row.reviewer = r.reviewer.as_ref().map(|x| (x.raised, x.valid, x.dropped, x.blockers));
        row.solved = oracle_ok(t, &cwd).await;
        if r.verdict == "pass" && !row.solved {
            row.verifier_false_negative = true;
        }
        if r.verdict == "error" {
            row.error = Some(r.reason);
        }
    } else {
        let mut e: Vec<(String, String)> = std::env::vars().collect();
        e.push(("OPENAI_BASE_URL".into(), cfg.endpoint.clone()));
        e.push(("OPENAI_MODEL".into(), cfg.model.clone()));
        e.push(("OPENAI_API_KEY".into(), env.get(&cfg.api_key_env).cloned().unwrap_or_else(|| "none".into())));
        let r = run(&format!("{qwen_cmd} -y -p {:?}", t.prompt), &cwd, RunOpts { timeout: Some(Duration::from_secs(t.timeout_s)), env: Some(e), ..Default::default() }).await;
        if r.code != Some(0) {
            let msg = if r.stderr.is_empty() { r.stdout } else { r.stderr };
            row.error = Some(msg.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect());
        }
        row.solved = oracle_ok(t, &cwd).await;
    }
    row.seconds = t0.elapsed().as_secs_f64();
    row
}

fn median(a: &[f64]) -> f64 {
    let mut s = a.to_vec();
    s.sort_by(|x, y| x.partial_cmp(y).unwrap());
    if s.is_empty() { 0.0 } else if s.len() % 2 == 1 { s[s.len() / 2] } else { (s[s.len() / 2 - 1] + s[s.len() / 2]) / 2.0 }
}

fn pct(n: f64, d: f64) -> String {
    if d == 0.0 { "n/a".into() } else { format!("{:.1}%", 100.0 * n / d) }
}

fn sum_u(a: impl Iterator<Item = Option<u64>>) -> u64 {
    a.map(|x| x.unwrap_or(0)).sum()
}

pub fn summarize(rows: &[EvalRow]) -> String {
    let mut runners: Vec<String> = Vec::new();
    for r in rows {
        if !runners.contains(&r.runner) {
            runners.push(r.runner.clone());
        }
    }
    let mut lines = vec![
        "| runner | tasks | pass rate | median s | tool calls | malformed | repaired | think leaks | verifier FN | avg rounds | tokens out |".to_string(),
        "|---|---|---|---|---|---|---|---|---|---|---|".to_string(),
    ];
    for rn in &runners {
        let rs: Vec<&EvalRow> = rows.iter().filter(|r| &r.runner == rn).collect();
        let is_fh = rn.starts_with("fh");
        let calls = sum_u(rs.iter().map(|r| r.tool_calls));
        let na = || "n/a".to_string();
        lines.push(format!(
            "| {} | {} | {} | {:.1} | {} | {} | {} | {} | {} | {} | {} |",
            rn,
            rs.len(),
            pct(rs.iter().filter(|r| r.solved).count() as f64, rs.len() as f64),
            median(&rs.iter().map(|r| r.seconds).collect::<Vec<_>>()),
            if is_fh { calls.to_string() } else { na() },
            if is_fh { pct(sum_u(rs.iter().map(|r| r.malformed)) as f64, calls as f64) } else { na() },
            if is_fh { pct(sum_u(rs.iter().map(|r| r.repaired)) as f64, calls as f64) } else { na() },
            if is_fh { sum_u(rs.iter().map(|r| r.think_leaks)).to_string() } else { na() },
            if is_fh { pct(rs.iter().filter(|r| r.verifier_false_negative).count() as f64, rs.len() as f64) } else { na() },
            if is_fh { format!("{:.2}", rs.iter().map(|r| r.rounds.unwrap_or(0)).sum::<usize>() as f64 / rs.len().max(1) as f64) } else { na() },
            if is_fh { sum_u(rs.iter().map(|r| r.tokens_out)).to_string() } else { na() },
        ));
    }
    let fh: Vec<&EvalRow> = rows.iter().filter(|r| r.runner == "fh").collect();
    if !fh.is_empty() {
        let raised: usize = fh.iter().filter_map(|r| r.reviewer).map(|r| r.0).sum();
        let dropped: usize = fh.iter().filter_map(|r| r.reviewer).map(|r| r.2).sum();
        let fp = fh.iter().filter(|r| r.verdict.as_deref() == Some("fail") && r.solved).count();
        lines.push(String::new());
        lines.push(format!("Verifier: verdict=fail while the oracle passes the final tree: {fp}/{}; reviewer findings raised {raised}, dropped as uncheckable {dropped} ({}).", fh.len(), pct(dropped as f64, raised as f64)));
        let acc: Vec<f64> = fh.iter().filter_map(|r| r.acceptance).collect();
        if !acc.is_empty() {
            let pf: Vec<f64> = fh.iter().filter_map(|r| r.prefix_hit).collect();
            lines.push(format!("MTP acceptance (mean over tasks): {:.1}%. Prefix-cache hit rate (mean): {:.1}%.", 100.0 * acc.iter().sum::<f64>() / acc.len() as f64, if pf.is_empty() { 0.0 } else { 100.0 * pf.iter().sum::<f64>() / pf.len() as f64 }));
        }
        lines.push(format!("Skill gate: mean {:.2} ms, 0 LLM calls.", fh.iter().filter_map(|r| r.gate_ms).sum::<f64>() / fh.len() as f64));
    }
    lines.join("\n")
}

pub struct EvalOptions {
    pub tasks: PathBuf,
    /// fh | qwen | both
    pub runner: String,
    pub out: PathBuf,
    pub limit: Option<usize>,
    pub repeat: usize,
    pub qwen_cmd: String,
}

pub async fn run_eval(cfg: &Config, env: &Env, o: EvalOptions) -> i32 {
    let mut tasks = load_tasks(&o.tasks);
    if let Some(l) = o.limit {
        tasks.truncate(l);
    }
    // both = fh vs plain Qwen Code; all = also fh-single (orchestration off) to measure what the workers add
    let runners: Vec<&str> = match o.runner.as_str() { "both" => vec!["fh", "qwen"], "all" => vec!["fh", "fh-single", "qwen"], "orch" => vec!["fh", "fh-single"], r => vec![r] };
    let stamp = chrono_stamp();
    let dir = o.out.join(format!("eval-{stamp}"));
    let _ = std::fs::create_dir_all(&dir);
    let mut rows: Vec<EvalRow> = Vec::new();
    for t in &tasks {
        for rep in 1..=o.repeat.max(1) {
            for rn in &runners {
                print!("{rn} {} #{rep} … ", t.id);
                let r = run_one(cfg, env, t, rn, rep, &o.qwen_cmd).await;
                println!("{} in {:.1}s{}", if r.solved { "solved" } else { "failed" }, r.seconds, r.error.as_ref().map(|e| format!(" ({})", e.chars().take(80).collect::<String>())).unwrap_or_default());
                rows.push(r);
                let _ = std::fs::write(dir.join("results.jsonl"), rows.iter().map(|x| x.to_json().to_string()).collect::<Vec<_>>().join("\n") + "\n");
            }
        }
    }
    let md = format!("# Eval {stamp}\n\nModel {} @ {}\n\n{}\n", cfg.model, cfg.endpoint, summarize(&rows));
    let _ = std::fs::write(dir.join("summary.md"), &md);
    println!("\n{md}\nSaved to {}", dir.display());
    if rows.iter().all(|r| r.solved) { 0 } else { 1 }
}

/// UTC timestamp like 20260930T004123Z (no chrono dependency).
pub fn chrono_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z", y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}
