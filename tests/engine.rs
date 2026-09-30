use fh::config::{load_config, Config, Env};
use fh::engine::*;
use fh::eval::corpus::builtin_tasks;
use fh::eval::runner::{run_one, summarize};
use fh::funnel::intake::Subtask;
use fh::skills::reuse::ReuseOffer;
use fh::skills::store::SkillStore;
use fh::testkit::{self, Mock, Scripted};
use fh::types::Mode;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

fn cfg_for(m: &Mock) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = m.url.clone();
    c.metrics_url = format!("{}/metrics", m.base);
    c.retries = 0;
    c.verify_rounds_normal = 2;
    c.verify_rounds_auto = 3;
    c
}

fn fixture(id: &str) -> (tempfile::TempDir, fh::eval::corpus::EvalTask) {
    let t = builtin_tasks().into_iter().find(|x| x.id == id).unwrap_or_else(|| panic!("missing task {id} (python3 required)"));
    let d = tempfile::tempdir().unwrap();
    for (f, b) in &t.files {
        let p = d.path().join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }
    assert!(Command::new("sh").arg("-c").arg("git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm base").current_dir(d.path()).status().unwrap().success());
    (d, t)
}

fn headless(log: Arc<Mutex<Vec<String>>>) -> Arc<dyn Io> {
    Arc::new(HeadlessIo { log: Some(Arc::new(move |m| log.lock().unwrap().push(m.to_string()))) })
}

/// Routes a request the way a real model would: structured stages by schema, agent turns by history.
fn brain(fix_old: &'static str, mine: Option<Value>) -> impl Fn(&Value) -> Scripted + Send + Sync + 'static {
    move |req| {
        let props = &req["response_format"]["json_schema"]["schema"]["properties"];
        if props.get("trivial").is_some() {
            return Scripted::json(json!({"trivial": true, "questions": [], "enriched": "Fix sum_range to be inclusive", "acceptance": ["tests pass"], "plan": [{"step": "fix loop bound", "files": ["mathx.py"]}], "assumptions": [], "subtasks": []}));
        }
        if props.get("verdict").is_some() {
            return Scripted::json(json!({"verdict": "pass", "findings": []}));
        }
        if props.get("create").is_some() {
            return Scripted::json(mine.clone().unwrap_or(json!({"create": false})));
        }
        let msgs = req["messages"].as_array().unwrap();
        let last = msgs.last().unwrap();
        if last["role"] == "tool" {
            return Scripted::text("Fixed the loop bound.");
        }
        if last["role"] == "user" && last["content"].as_str().unwrap_or("").contains("Verification round") {
            return Scripted::call("edit", json!({"path": "mathx.py", "old_text": "in range(a, b)", "new_text": "in range(a, b + 1)"}));
        }
        Scripted::call("edit", json!({"path": "mathx.py", "old_text": fix_old, "new_text": "in range(a, b + 1)"}))
    }
}

async fn run(m: &Mock, d: &Path, cfg: Config, io: Arc<dyn Io>, store: SkillStore, task: &str, o: TaskOptions) -> TaskResult {
    Engine::new(cfg, Env::new(), io, d, store).run_task(task, o).await
}
fn auto() -> TaskOptions {
    TaskOptions { auto: true, approval: Mode::Yolo, no_mine: true, ..Default::default() }
}

#[tokio::test]
async fn auto_mode_plan_work_verify_passes_gate_has_no_llm_call_and_prefix_is_stable() {
    let m = testkit::start(0, None).await;
    m.set_fallback(brain("in range(a, b)", None));
    let (d, _) = fixture("py-off-by-one");
    let log = Arc::new(Mutex::new(vec![]));
    let r = run(&m, d.path(), cfg_for(&m), headless(log.clone()), SkillStore::in_memory(), "fix sum_range", auto()).await;
    assert_eq!(r.verdict, "pass", "{}", r.reason);
    assert!(r.final_text.starts_with("Verified"));
    assert!(std::fs::read_to_string(d.path().join("mathx.py")).unwrap().contains("b + 1"));
    assert_eq!(r.gate.as_ref().unwrap().1, 0);
    assert!(r.gate.as_ref().unwrap().0 < 50.0);
    assert_eq!((r.rounds, r.changed.clone()), (1, vec!["mathx.py".to_string()]));
    assert!(r.llm.requests >= 3);
    assert!(log.lock().unwrap().iter().any(|l| l.contains("round 1: pass")));
    let reqs = m.requests();
    assert!(reqs[0]["response_format"]["json_schema"]["schema"]["properties"].get("trivial").is_some());
    let sys: std::collections::HashSet<String> = reqs.iter().filter(|q| q.get("response_format").is_none()).map(|q| q["messages"][0]["content"].as_str().unwrap().to_string()).collect();
    assert_eq!(sys.len(), 1);
}

#[tokio::test]
async fn failing_verification_then_fix_round_then_pass() {
    let m = testkit::start(0, None).await;
    let inner = brain("in range(a, b)", None);
    m.set_fallback(move |req| {
        let last = req["messages"].as_array().unwrap().last().unwrap().clone();
        if req.get("response_format").is_none() && last["role"] == "user" && !last["content"].as_str().unwrap_or("").contains("Verification round") {
            return Scripted::call("edit", json!({"path": "mathx.py", "old_text": "total = 0", "new_text": "total = 0  # start"})); // harmless first attempt
        }
        inner(req)
    });
    let (d, _) = fixture("py-off-by-one");
    let r = run(&m, d.path(), cfg_for(&m), headless(Default::default()), SkillStore::in_memory(), "fix sum_range", auto()).await;
    assert_eq!((r.verdict.as_str(), r.rounds), ("pass", 2));
}

#[tokio::test]
async fn unfixable_rolls_back_saves_rejected_patch_and_says_not_delivered() {
    let m = testkit::start(0, None).await;
    let inner = brain("in range(a, b)", None);
    m.set_fallback(move |req| {
        let last = req["messages"].as_array().unwrap().last().unwrap().clone();
        if req.get("response_format").is_none() && last["role"] != "tool" {
            return Scripted::call("edit", json!({"path": "mathx.py", "old_text": "total = 0", "new_text": "total = 1"}));
        }
        inner(req)
    });
    let (d, _) = fixture("py-off-by-one");
    let original = std::fs::read_to_string(d.path().join("mathx.py")).unwrap();
    let r = run(&m, d.path(), cfg_for(&m), headless(Default::default()), SkillStore::in_memory(), "fix sum_range", auto()).await;
    assert_eq!(r.verdict, "fail");
    assert!(r.rolled_back && r.final_text.contains("NOT delivered"));
    assert_eq!(std::fs::read_to_string(d.path().join("mathx.py")).unwrap(), original);
    assert!(d.path().join(r.rejected_patch.unwrap()).exists());
}

#[tokio::test]
async fn repo_without_tests_is_unverified_never_silently_passed() {
    let m = testkit::start(0, None).await;
    m.set_fallback(|req| {
        let p = &req["response_format"]["json_schema"]["schema"]["properties"];
        if p.get("trivial").is_some() {
            return Scripted::json(json!({"trivial": true, "questions": [], "enriched": "edit note", "acceptance": [], "plan": [{"step": "s", "files": ["a.txt"]}], "assumptions": [], "subtasks": []}));
        }
        if p.get("verdict").is_some() {
            return Scripted::json(json!({"verdict": "pass", "findings": []}));
        }
        if req["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" {
            return Scripted::text("done");
        }
        Scripted::call("edit", json!({"path": "a.txt", "old_text": "x", "new_text": "y"}))
    });
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "x").unwrap();
    assert!(Command::new("sh").arg("-c").arg("git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm b").current_dir(d.path()).status().unwrap().success());
    let r = run(&m, d.path(), cfg_for(&m), headless(Default::default()), SkillStore::in_memory(), "edit", auto()).await;
    assert_eq!(r.verdict, "unverified");
    assert!(r.final_text.contains("UNVERIFIED"));
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "y");
}

struct GuidedIo {
    asked: Mutex<Vec<Vec<String>>>,
    plans: Mutex<Vec<String>>,
    approve: bool,
}
#[async_trait]
impl Io for GuidedIo {
    fn notice(&self, _: NoticeKind, _: &str) {}
    async fn ask_questions(&self, q: Vec<String>) -> Vec<String> {
        self.asked.lock().unwrap().push(q);
        vec!["inclusive".into()]
    }
    async fn approve_plan(&self, p: &str, _: bool) -> PlanDecision {
        self.plans.lock().unwrap().push(p.to_string());
        PlanDecision { ok: self.approve, feedback: None }
    }
    async fn confirm(&self, _: &str, _: &Value) -> bool {
        true
    }
    async fn offer_reuse(&self, _: &[ReuseOffer]) -> Vec<(String, Vec<String>)> {
        vec![]
    }
}

#[tokio::test]
async fn guided_mode_asks_questions_shows_plan_and_rejection_aborts_before_any_edit() {
    let m = testkit::start(0, None).await;
    let calls = Arc::new(Mutex::new(0));
    let (c2, b) = (calls.clone(), brain("in range(a, b)", None));
    m.set_fallback(move |req| {
        if req["response_format"]["json_schema"]["schema"]["properties"].get("trivial").is_some() {
            let mut n = c2.lock().unwrap();
            *n += 1;
            return Scripted::json(if *n == 1 {
                json!({"trivial": false, "questions": ["Inclusive or exclusive?"], "enriched": "x", "acceptance": [], "plan": [], "assumptions": [], "subtasks": []})
            } else {
                json!({"trivial": true, "questions": ["ignored"], "enriched": "Make sum_range inclusive", "acceptance": ["tests pass"], "plan": [{"step": "fix", "files": ["mathx.py"]}], "assumptions": [], "subtasks": []})
            });
        }
        b(req)
    });
    let (d, _) = fixture("py-off-by-one");
    let io = Arc::new(GuidedIo { asked: Mutex::new(vec![]), plans: Mutex::new(vec![]), approve: true });
    let r = run(&m, d.path(), cfg_for(&m), io.clone(), SkillStore::in_memory(), "fix", TaskOptions { approval: Mode::AutoEdit, no_mine: true, ..Default::default() }).await;
    assert_eq!(io.asked.lock().unwrap().clone(), vec![vec!["Inclusive or exclusive?".to_string()]]);
    assert_eq!(io.plans.lock().unwrap().len(), 1);
    assert!(io.plans.lock().unwrap()[0].contains("fix"));
    assert_eq!(r.verdict, "pass");
    let second = m.requests()[1]["messages"].to_string();
    assert!(second.contains("Q: Inclusive or exclusive?") && second.contains("A: inclusive"));

    let m2 = testkit::start(0, None).await;
    m2.set_fallback(brain("in range(a, b)", None));
    let (d2, _) = fixture("py-off-by-one");
    let orig = std::fs::read_to_string(d2.path().join("mathx.py")).unwrap();
    let io2 = Arc::new(GuidedIo { asked: Mutex::new(vec![]), plans: Mutex::new(vec![]), approve: false });
    let r2 = run(&m2, d2.path(), cfg_for(&m2), io2, SkillStore::in_memory(), "fix", TaskOptions { approval: Mode::AutoEdit, no_mine: true, ..Default::default() }).await;
    assert_eq!(r2.verdict, "aborted");
    assert_eq!(std::fs::read_to_string(d2.path().join("mathx.py")).unwrap(), orig);
}

#[tokio::test]
async fn parallel_workers_on_disjoint_files_then_merged_verification() {
    let (d, _) = fixture("py-two-modules");
    let m = testkit::start(0, None).await;
    m.set_fallback(|req| {
        let p = &req["response_format"]["json_schema"]["schema"]["properties"];
        if p.get("trivial").is_some() {
            return Scripted::json(json!({"trivial": false, "questions": [], "enriched": "implement both", "acceptance": ["tests pass"],
                "plan": [{"step": "a", "files": ["server/validate.py"]}, {"step": "b", "files": ["client/format.py"]}], "assumptions": [],
                "subtasks": [{"id": "server", "goal": "implement is_email in server/validate.py", "files": ["server/**"], "deps": []}, {"id": "client", "goal": "implement format_cents in client/format.py", "files": ["client/**"], "deps": []}]}));
        }
        if p.get("verdict").is_some() {
            return Scripted::json(json!({"verdict": "pass", "findings": []}));
        }
        let msgs = req["messages"].as_array().unwrap();
        let user = msgs.iter().find(|x| x["role"] == "user").unwrap()["content"].as_str().unwrap().to_string();
        if msgs.last().unwrap()["role"] == "tool" {
            return Scripted::text("done");
        }
        if user.contains("(server)") {
            return Scripted::call("edit", json!({"path": "server/validate.py", "old_text": "raise NotImplementedError", "new_text": "return isinstance(s, str) and s.count('@') == 1 and ' ' not in s and s.split('@')[0] != '' and '.' in s.split('@')[1]"}));
        }
        if user.contains("(client)") {
            return Scripted::call("edit", json!({"path": "client/format.py", "old_text": "raise NotImplementedError", "new_text": "return '${:,.2f}'.format(n / 100)"}));
        }
        Scripted::text("noop")
    });
    let log = Arc::new(Mutex::new(vec![]));
    let mut c = cfg_for(&m);
    c.max_concurrency = 2;
    let r = run(&m, d.path(), c, headless(log.clone()), SkillStore::in_memory(), "two changes", auto()).await;
    assert_eq!(r.verdict, "pass", "{:?}", r.verify.as_ref().and_then(|v| v.rounds.last()).map(|x| x.checks.iter().map(|c| (c.name.clone(), c.detail.clone())).collect::<Vec<_>>()));
    assert_eq!(r.workers.len(), 2);
    assert!(log.lock().unwrap().iter().any(|l| l.contains("wave 1/1: 2 worker")));
    let mut ch = r.changed.clone();
    ch.sort();
    assert_eq!(ch, vec!["client/format.py".to_string(), "server/validate.py".to_string()]);
}

#[tokio::test]
async fn post_delivery_learning_creates_a_hidden_project_skill_that_the_next_task_uses() {
    let m = testkit::start(0, None).await;
    let body = "- Keep loop bounds inclusive when the API contract says inclusive.\n- Add a boundary test for equal endpoints.\n- Prefer widening the range end over adjusting the start.";
    m.set_fallback(brain("in range(a, b)", Some(json!({"create": true, "name": "Inclusive ranges", "summary": "range helpers are inclusive", "keywords": ["range", "sum", "inclusive", "loop"], "body": body}))));
    let (d, _) = fixture("py-off-by-one");
    let store = SkillStore::in_memory();
    let engine = Engine::new(cfg_for(&m), Env::new(), headless(Default::default()), d.path(), store.clone());
    let r = engine.run_task("fix sum_range", TaskOptions { auto: true, approval: Mode::Yolo, ..Default::default() }).await;
    engine.drain(10_000).await;
    assert_eq!(r.verdict, "pass");
    assert!(store.activity(20).iter().any(|a| a.kind == "created" && a.skill == "Inclusive ranges"));
    assert_eq!(store.user_skills(None).len(), 1);
    assert!(!format!("{}{:?}", r.final_text, r.plan).contains("Keep loop bounds")); // hidden from the user
    assert!(Command::new("sh").arg("-c").arg("git checkout -q -- . && git clean -fdq").current_dir(d.path()).status().unwrap().success());
    m.clear_requests();
    let r2 = engine.run_task("fix the sum_range inclusive loop", auto()).await;
    assert!(r2.skills_used.contains(&"Inclusive ranges".to_string()), "{:?}", r2.skills_used);
    let agent_req = m.requests().into_iter().find(|q| q.get("response_format").is_none()).unwrap();
    assert!(agent_req["messages"].to_string().contains("Keep loop bounds inclusive"));
}

#[tokio::test]
async fn eval_corpus_fails_before_and_run_one_solves_a_task_through_the_engine() {
    for t in builtin_tasks() {
        let d = tempfile::tempdir().unwrap();
        for (f, b) in &t.files {
            let p = d.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b).unwrap();
        }
        let ok = Command::new("sh").arg("-c").arg(&t.oracle).current_dir(d.path()).env_remove("NODE_TEST_CONTEXT").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap().success();
        assert!(!ok, "{} must fail before the fix", t.id);
    }
    let m = testkit::start(0, None).await;
    m.set_fallback(brain("in range(a, b)", None));
    let t = builtin_tasks().into_iter().find(|t| t.id == "py-off-by-one").unwrap();
    let row = run_one(&cfg_for(&m), &Env::new(), &t, "fh", 1, "qwen").await;
    assert!(row.solved);
    assert_eq!(row.verdict.as_deref(), Some("pass"));
    assert_eq!(row.malformed, Some(0));
    assert!(summarize(&[row]).contains("| fh | 1 | 100.0% |"));
    let _ = PathBuf::new();
    let _ = Subtask { id: String::new(), goal: String::new(), files: vec![], deps: vec![] };
}
