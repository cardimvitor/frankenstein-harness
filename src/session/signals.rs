//! Runtime false-positive / false-negative signals for the verifier, and opt-in local telemetry.
//! - "undid_pass": the user ran `fh undo` after a task that passed (verifier let a bad change through).
//! - "kept_rejected": the user applied a rejected patch with `fh keep` (verifier blocked a good change).
//! Everything stays in the local data dir; nothing leaves the machine.
use crate::config::{data_dir, Env};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn append(path: &Path, v: &Value) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{v}");
    }
}

/// `.fh/last_task.json`: what `fh undo` and `fh keep` need to interpret the user's action.
pub fn write_last_task(cwd: &Path, verdict: &str, rejected_patch: Option<&str>, changed: &[String]) {
    let dir = cwd.join(".fh");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("last_task.json"), json!({"ts": now(), "verdict": verdict, "rejectedPatch": rejected_patch, "changed": changed, "handled": false}).to_string());
}

pub fn read_last_task(cwd: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(cwd.join(".fh").join("last_task.json")).ok()?).ok()
}

pub fn mark_handled(cwd: &Path) {
    if let Some(mut v) = read_last_task(cwd) {
        v["handled"] = json!(true);
        let _ = std::fs::write(cwd.join(".fh").join("last_task.json"), v.to_string());
    }
}

pub fn record_signal(env: &Env, project_id: &str, kind: &str) {
    append(&data_dir(env).join("signals.jsonl"), &json!({"ts": now(), "project": project_id, "kind": kind}));
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignalCounts {
    pub undid_pass: usize,
    pub kept_rejected: usize,
}

pub fn signal_counts(env: &Env) -> SignalCounts {
    let mut c = SignalCounts::default();
    for l in std::fs::read_to_string(data_dir(env).join("signals.jsonl")).unwrap_or_default().lines() {
        match serde_json::from_str::<Value>(l).ok().and_then(|v| v["kind"].as_str().map(|s| s.to_string())).as_deref() {
            Some("undid_pass") => c.undid_pass += 1,
            Some("kept_rejected") => c.kept_rejected += 1,
            _ => {}
        }
    }
    c
}

/// Opt-in (`telemetry: true` in the config): one JSON line per task in the local data dir.
pub fn record_task(env: &Env, v: Value) {
    append(&data_dir(env).join("telemetry.jsonl"), &v);
}

#[derive(Clone, Debug, Default)]
pub struct TelemetrySummary {
    pub tasks: usize,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub avg_acceptance: Option<f64>,
    pub repaired_calls: u64,
}

pub fn telemetry_summary(env: &Env) -> TelemetrySummary {
    let rows: Vec<Value> = std::fs::read_to_string(data_dir(env).join("telemetry.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let mut ms: Vec<u64> = rows.iter().filter_map(|r| r["totalMs"].as_u64()).collect();
    ms.sort();
    let pct = |p: f64| if ms.is_empty() { 0 } else { ms[(((ms.len() - 1) as f64) * p).round() as usize] };
    let acc: Vec<f64> = rows.iter().filter_map(|r| r["acceptanceRate"].as_f64()).collect();
    TelemetrySummary { tasks: rows.len(), p50_ms: pct(0.5), p95_ms: pct(0.95), avg_acceptance: if acc.is_empty() { None } else { Some(acc.iter().sum::<f64>() / acc.len() as f64) }, repaired_calls: rows.iter().filter_map(|r| r["repaired"].as_u64()).sum() }
}
