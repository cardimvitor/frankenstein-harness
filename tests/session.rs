use fh::config::{load_config, Config, Env};
use fh::engine::*;
use fh::eval::corpus::builtin_tasks;
use fh::session::log;
use fh::session::signals::*;
use fh::skills::store::SkillStore;
use fh::testkit::{self, Mock, Scripted};
use fh::types::Mode;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

fn cfg_for(m: &Mock) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = m.url.clone();
    c.metrics_url = format!("{}/metrics", m.base);
    c.retries = 0;
    c.verify_rounds_auto = 2;
    c
}

fn fixture() -> tempfile::TempDir {
    let t = builtin_tasks().into_iter().find(|x| x.id == "py-off-by-one").expect("python3 required");
    let d = tempfile::tempdir().unwrap();
    for (f, b) in &t.files {
        let p = d.path().join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }
    assert!(Command::new("sh").arg("-c").arg("git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm base").current_dir(d.path()).status().unwrap().success());
    d
}

fn env_home(h: &Path) -> Env {
    let mut e = Env::new();
    e.insert("FH_HOME".into(), h.to_string_lossy().to_string());
    e
}

fn plan_json() -> Value {
    json!({"trivial": true, "questions": [], "enriched": "Fix sum_range to be inclusive", "acceptance": ["tests pass"], "plan": [{"step": "fix loop bound", "files": ["mathx.py"]}], "assumptions": [], "subtasks": []})
}

fn auto() -> TaskOptions {
    TaskOptions { auto: true, approval: Mode::Yolo, no_mine: true, ..Default::default() }
}

#[tokio::test]
async fn interrupted_run_resumes_with_stored_plan_checkpoint_and_history() {
    let home = tempfile::tempdir().unwrap();
    let env = env_home(home.path());
    let d = fixture();
    let m = testkit::start(0, None).await;
    // first run: planning works, the agent reads a file, then the process "dies" during the next model call
    m.set_fallback(|req| {
        if req["response_format"]["json_schema"]["schema"]["properties"].get("trivial").is_some() {
            return Scripted::json(plan_json());
        }
        let last = req["messages"].as_array().unwrap().last().unwrap().clone();
        if last["role"] == "tool" {
            return Scripted { delay_ms: 60_000, content: Some("never".into()), ..Default::default() };
        }
        Scripted::call("read_file", json!({"path": "mathx.py"}))
    });
    let eng = Engine::new(cfg_for(&m), env.clone(), Arc::new(HeadlessIo { log: None }), d.path(), SkillStore::in_memory());
    let _ = tokio::time::timeout(Duration::from_millis(2500), eng.run_task("fix sum_range", auto())).await;
    drop(eng);

    let fp = fh::fingerprint::fingerprint(d.path());
    let s = log::load(&env, &fp.project_id, None).expect("session recorded");
    assert!(s.verdict.is_none(), "interrupted session has no result");
    assert_eq!(s.task, "fix sum_range");
    let base = s.checkpoint.clone().unwrap();
    assert!(!base.is_empty() && s.plan.is_some());
    let hist = s.history.clone().expect("history saved after the first step");
    assert!(hist.iter().any(|h| h.role == "tool"));

    // resume: no planning request is made; the same checkpoint is the base; the task completes and verifies
    m.clear_requests();
    m.set_fallback(|req| {
        assert!(req["response_format"]["json_schema"]["schema"]["properties"].get("trivial").is_none(), "must not re-plan");
        if req["response_format"]["json_schema"]["schema"]["properties"].get("verdict").is_some() {
            return Scripted::json(json!({"verdict": "pass", "findings": []}));
        }
        let last = req["messages"].as_array().unwrap().last().unwrap().clone();
        if last["role"] == "tool" {
            return Scripted::text("Fixed.");
        }
        Scripted::call("edit", json!({"path": "mathx.py", "old_text": "in range(a, b)", "new_text": "in range(a, b + 1)"}))
    });
    let eng = Engine::new(cfg_for(&m), env.clone(), Arc::new(HeadlessIo { log: None }), d.path(), SkillStore::in_memory());
    let mut o = auto();
    o.resume = Some(ResumeState { session_id: s.id.clone(), plan: s.plan.clone().unwrap(), base: base.clone(), history: Some(hist) });
    let r = eng.run_task("fix sum_range", o).await;
    assert_eq!(r.verdict, "pass", "{}", r.reason);
    // the agent continued from its history: the first request carries the earlier tool result
    let first = &m.requests()[0]["messages"];
    assert!(first.as_array().unwrap().iter().any(|x| x["role"] == "tool"));
    let s2 = log::load(&env, &fp.project_id, Some(&s.id)).unwrap();
    assert_eq!(s2.verdict.as_deref(), Some("pass"));
    assert_eq!(s2.checkpoint.as_deref(), Some(base.as_str()));
}

#[tokio::test]
async fn resume_refuses_when_checkpoint_is_gone() {
    let home = tempfile::tempdir().unwrap();
    let d = fixture();
    let m = testkit::start(0, None).await;
    m.set_fallback(|_| Scripted::json(plan_json()));
    let eng = Engine::new(cfg_for(&m), env_home(home.path()), Arc::new(HeadlessIo { log: None }), d.path(), SkillStore::in_memory());
    let mut o = auto();
    o.resume = Some(ResumeState { session_id: "x".into(), plan: serde_json::from_value(plan_json()).unwrap(), base: "deadbeef".repeat(5), history: None });
    let r = eng.run_task("t", o).await;
    assert_eq!(r.verdict, "error");
    assert!(r.reason.contains("checkpoint"));
}

#[test]
fn signals_and_telemetry_are_counted() {
    let home = tempfile::tempdir().unwrap();
    let env = env_home(home.path());
    assert_eq!(signal_counts(&env), SignalCounts::default());
    record_signal(&env, "p", "undid_pass");
    record_signal(&env, "p", "undid_pass");
    record_signal(&env, "p", "kept_rejected");
    assert_eq!(signal_counts(&env), SignalCounts { undid_pass: 2, kept_rejected: 1 });
    for ms in [1000, 2000, 3000, 4000, 10000] {
        record_task(&env, json!({"totalMs": ms, "acceptanceRate": 0.6, "repaired": 1}));
    }
    let t = telemetry_summary(&env);
    assert_eq!((t.tasks, t.p50_ms, t.p95_ms, t.repaired_calls), (5, 3000, 10000, 5));
    assert!((t.avg_acceptance.unwrap() - 0.6).abs() < 1e-9);
    let ws = tempfile::tempdir().unwrap();
    write_last_task(ws.path(), "pass", None, &["a".to_string()]);
    assert_eq!(read_last_task(ws.path()).unwrap()["verdict"], "pass");
    mark_handled(ws.path());
    assert_eq!(read_last_task(ws.path()).unwrap()["handled"], true);
}
