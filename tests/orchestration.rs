use fh::config::{load_config, Config, Env};
use fh::funnel::intake::Subtask;
use fh::llm::client::LlmClient;
use fh::orchestrator::governor::Governor;
use fh::orchestrator::master::{run_workers, MasterOptions};
use fh::testkit::{self, Mock, Scripted};
use fh::types::Mode;
use serde_json::{json, Value};
use std::path::Path;

fn cfg(m: &Mock) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = m.url.clone();
    c.retries = 0;
    c
}

fn opts(m: &Mock, cwd: &Path, budget: u64) -> MasterOptions {
    MasterOptions {
        llm: LlmClient::new(cfg(m), Env::new()),
        cwd: cwd.to_path_buf(),
        mode: Mode::Yolo,
        context: "ctx".into(),
        governor: Governor::new(3, None, 2),
        cancel: None,
        context_window: 131072,
        max_steps: 10,
        notice: None,
        wrap_shell: None,
        confirm: None,
        extra_tools: vec![],
        task_budget: budget,
    }
}

fn st(id: &str, files: &[&str]) -> Subtask {
    Subtask { id: id.into(), goal: format!("do {id}"), files: files.iter().map(|s| s.to_string()).collect(), deps: vec![] }
}

fn first_user(req: &Value) -> String {
    req["messages"].as_array().unwrap().iter().find(|m| m["role"] == "user").map(|m| m["content"].as_str().unwrap_or("").to_string()).unwrap_or_default()
}

fn has_tool_result(req: &Value) -> bool {
    req["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool")
}

#[tokio::test]
async fn blocked_worker_request_is_routed_to_the_owning_worker() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "one").unwrap();
    std::fs::write(d.path().join("b.txt"), "one").unwrap();
    let m = testkit::start(0, None).await;
    m.set_fallback(|req| {
        let task = first_user(req);
        let done = has_tool_result(req);
        if task.contains("Requests from other workers") {
            // the owner (s2) applies the requested change
            return if done { Scripted::text("applied the request") } else { Scripted::call("edit", json!({"path": "b.txt", "old_text": "one", "new_text": "two"})) };
        }
        if task.contains("Your subtask (s1)") {
            return if done { Scripted::text("s1 done") } else { Scripted::call("request_edit", json!({"path": "b.txt", "reason": "change 'one' to 'two'"})) };
        }
        Scripted::text("s2 done")
    });
    let subs = vec![st("s1", &["a.txt"]), st("s2", &["b.txt"])];
    let r = run_workers("goal", &subs, &opts(&m, d.path(), 0)).await;
    assert_eq!(std::fs::read_to_string(d.path().join("b.txt")).unwrap(), "two", "{r:?}");
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "one");
    assert_eq!(r.len(), 3, "two workers plus one follow-up: {r:?}");
    let follow = r.iter().find(|w| w.id.contains("follow-up")).unwrap();
    assert!(follow.id.starts_with("s2") && follow.touched == vec!["b.txt"], "{follow:?}");
    assert_eq!(r.iter().find(|w| w.id == "s1").unwrap().blocked.len(), 1);
}

#[tokio::test]
async fn request_for_an_unowned_file_goes_to_an_extra_worker() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "x").unwrap();
    std::fs::write(d.path().join("b.txt"), "x").unwrap();
    std::fs::write(d.path().join("shared.txt"), "old").unwrap();
    let m = testkit::start(0, None).await;
    m.set_fallback(|req| {
        let task = first_user(req);
        let done = has_tool_result(req);
        if task.contains("Requests from other workers") {
            return if done { Scripted::text("ok") } else { Scripted::call("edit", json!({"path": "shared.txt", "old_text": "old", "new_text": "new"})) };
        }
        if task.contains("Your subtask (s1)") && !done {
            return Scripted::call("request_edit", json!({"path": "shared.txt", "reason": "update it"}));
        }
        Scripted::text("done")
    });
    let subs = vec![st("s1", &["a.txt"]), st("s2", &["b.txt"])];
    let r = run_workers("goal", &subs, &opts(&m, d.path(), 0)).await;
    assert_eq!(std::fs::read_to_string(d.path().join("shared.txt")).unwrap(), "new", "{r:?}");
    assert!(r.iter().any(|w| w.id.starts_with("extra-") && w.id.contains("follow-up")), "{r:?}");
}

#[tokio::test]
async fn worker_stops_when_its_share_of_the_token_budget_is_used() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "x").unwrap();
    let m = testkit::start(0, None).await;
    // every step reports a large usage; the mock returns usage from the scripted response if present
    m.set_fallback(|_| Scripted { usage: Some((15_000, 10_000)), ..Scripted::call("list_files", json!({})) });
    let subs = vec![st("s1", &["a.txt"])];
    // budget 100k -> 60k share for the single worker: stops after a few 25k steps, not at max_steps
    let r = run_workers("goal", &subs, &opts(&m, d.path(), 100_000)).await;
    assert_eq!(r[0].stopped.as_str(), "budget", "{r:?}");
    assert!(r[0].steps <= 4 && r[0].tokens >= 60_000, "{r:?}");
}
