use fh::config::{load_config, Config, Env};
use fh::delegate::{prepare, verification_task, worker_config};
use fh::testkit::{self, Scripted};
use std::path::Path;

const EDIT: &str = "mathx.py\n<<<<<<< SEARCH\n    for i in range(a, b):\n=======\n    for i in range(a, b + 1):\n>>>>>>> REPLACE\n";

fn cfg(url: &str) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = url.to_string();
    c.retries = 0;
    c
}

fn env_for(worker_url: &str) -> Env {
    let mut e = Env::new();
    e.insert("FH_WORKER_ENDPOINT".into(), worker_url.into());
    e.insert("FH_WORKER_MODEL".into(), "worker-9b".into());
    e
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("mathx.py"), "def sum_range(a, b):\n    total = 0\n    for i in range(a, b):\n        total += i\n    return total\n").unwrap();
    d
}

#[test]
fn worker_config_needs_both_settings_and_keeps_the_main_ones() {
    let c = cfg("http://main/v1");
    assert!(worker_config(&c, &Env::new()).is_err());
    let w = worker_config(&c, &env_for("http://worker:8002/v1/")).unwrap();
    assert_eq!((w.endpoint.as_str(), w.model.as_str()), ("http://worker:8002/v1", "worker-9b"));
    assert_eq!(w.metrics_url, "http://worker:8002/metrics");
    assert_eq!(c.endpoint, "http://main/v1");
}

#[tokio::test]
async fn planner_writes_the_order_the_worker_patches_once_and_each_model_gets_its_own_calls() {
    let main = testkit::start(0, None).await;
    let work = testkit::start(0, None).await;
    main.push(Scripted::text("Change the loop bound in sum_range to b + 1."));
    work.push(Scripted::text(EDIT));
    let d = repo();
    let env = env_for(&work.url);
    let c = cfg(&main.url);
    let w = worker_config(&c, &env).unwrap();
    let p = prepare(&c, &w, &env, d.path(), "Fix the off-by-one in mathx.py").await;
    assert!(p.error.is_none(), "{:?}", p.error);
    assert_eq!(p.changed, vec!["mathx.py".to_string()]);
    assert!(std::fs::read_to_string(d.path().join("mathx.py")).unwrap().contains("range(a, b + 1)"));
    assert_eq!((p.planner.requests, p.worker.requests), (1, 1));
    let (mreq, wreq) = (main.requests(), work.requests());
    assert_eq!((mreq.len(), wreq.len()), (1, 1));
    assert!(wreq[0]["messages"].to_string().contains("Change the loop bound"), "the worker sees the work order");
    assert!(!mreq[0]["messages"].to_string().contains("SEARCH/REPLACE blocks, one block"), "the planner does not get the worker's format");
    assert!(wreq[0].get("tools").is_none());
    let t = verification_task("Fix it", &p);
    assert!(t.contains("mathx.py") && t.contains("will not be called again"));
}

#[tokio::test]
async fn a_dead_worker_is_an_error_and_a_reply_without_edits_leaves_the_workspace_alone() {
    let main = testkit::start(0, None).await;
    let work = testkit::start(0, None).await;
    main.push(Scripted::text("plan"));
    work.push(Scripted::text("I would change the loop."));
    let d = repo();
    let env = env_for(&work.url);
    let c = cfg(&main.url);
    let w = worker_config(&c, &env).unwrap();
    let p = prepare(&c, &w, &env, d.path(), "fix").await;
    assert!(p.error.is_none() && p.changed.is_empty() && p.edits == 0);
    assert!(verification_task("fix", &p).contains("no applicable edits"));
    main.push(Scripted::text("plan"));
    let env2 = env_for("http://127.0.0.1:9/v1");
    let w2 = worker_config(&c, &env2).unwrap();
    let p2 = prepare(&c, &w2, &env2, d.path(), "fix").await;
    assert!(p2.error.as_deref().unwrap_or("").starts_with("worker"), "{:?}", p2.error);
    assert_eq!(p2.worker.requests, 0);
}
