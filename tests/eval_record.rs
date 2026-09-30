use fh::eval::record::{record_task, RecordOptions};
use fh::eval::runner::load_tasks;
use std::path::Path;
use std::process::Command;

fn sh(cwd: &Path, c: &str) {
    assert!(Command::new("sh").arg("-c").arg(c).current_dir(cwd).output().unwrap().status.success(), "{c}");
}

fn opts(id: &str, out: &Path) -> RecordOptions {
    RecordOptions { id: id.into(), prompt: "Fix sum_range so it is inclusive.".into(), oracle: "python3 -m unittest discover -q".into(), out: out.to_path_buf(), base: Some("base".into()), solution: Some("solution".into()), setup: None, force: false }
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("mathx.py"), "def sum_range(a, b):\n    return sum(range(a, b))\n").unwrap();
    std::fs::write(d.path().join("test_mathx.py"), "import unittest\nfrom mathx import sum_range\n\nclass T(unittest.TestCase):\n    def test_it(self):\n        self.assertEqual(sum_range(1, 4), 10)\n").unwrap();
    sh(d.path(), "git init -q -b main && git config user.email a@b && git config user.name t && git add -A && git commit -qm base && git tag base");
    std::fs::write(d.path().join("mathx.py"), "def sum_range(a, b):\n    return sum(range(a, b + 1))\n").unwrap();
    sh(d.path(), "git commit -qam fix && git tag solution && git checkout -q base");
    d
}

#[tokio::test]
async fn record_validates_the_task_and_writes_a_loadable_directory() {
    let d = repo();
    let out = tempfile::tempdir().unwrap();
    let r = record_task(d.path(), opts("inclusive-range", out.path())).await.unwrap();
    assert!(r.oracle_fails_on_base);
    assert_eq!(r.oracle_passes_with_solution, Some(true));
    let t = out.path().join("inclusive-range");
    assert!(std::fs::read_to_string(t.join("repo/mathx.py")).unwrap().contains("range(a, b))"), "repo/ holds the BASE state");
    assert!(!t.join("repo/.git").exists());
    let patch = std::fs::read_to_string(t.join("solution.patch")).unwrap();
    assert!(patch.contains("b + 1"));
    // the runner loads it like any hand-written task
    let tasks = load_tasks(out.path());
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "inclusive-range");
    assert_eq!(tasks[0].oracle, "python3 -m unittest discover -q");
    // recording again needs --force
    assert!(record_task(d.path(), opts("inclusive-range", out.path())).await.unwrap_err().contains("already exists"));
    let mut o = opts("inclusive-range", out.path());
    o.force = true;
    assert!(record_task(d.path(), o).await.is_ok());
}

#[tokio::test]
async fn record_refuses_tasks_that_cannot_discriminate() {
    let d = repo();
    let out = tempfile::tempdir().unwrap();
    // an oracle that always passes
    let mut o = opts("trivial", out.path());
    o.oracle = "true".into();
    assert!(record_task(d.path(), o).await.unwrap_err().contains("already passes"));
    // an oracle that never passes, even with the solution
    let mut o = opts("impossible", out.path());
    o.oracle = "false".into();
    assert!(record_task(d.path(), o).await.unwrap_err().contains("does not pass"));
    // bad ids and revisions
    assert!(record_task(d.path(), opts("../evil", out.path())).await.is_err());
    let mut o = opts("x", out.path());
    o.solution = Some("no-such-rev".into());
    assert!(record_task(d.path(), o).await.unwrap_err().contains("unknown revision"));
    // without a solution the task is recorded (only the base failure is checked)
    let mut o = opts("no-solution", out.path());
    o.solution = None;
    let r = record_task(d.path(), o).await.unwrap();
    assert_eq!(r.oracle_passes_with_solution, None);
    assert!(!out.path().join("no-solution/solution.patch").exists());
}

#[test]
fn oracle_file_and_setup_are_loaded_from_task_json() {
    let out = tempfile::tempdir().unwrap();
    let t = out.path().join("t1");
    std::fs::create_dir_all(t.join("repo")).unwrap();
    std::fs::write(t.join("oracle.sh"), "python3 -m pytest -q tests/x.py\n").unwrap();
    std::fs::write(t.join("task.json"), r#"{"id":"t1","prompt":"p","oracleFile":"oracle.sh","setup":"pip install -e .","oracleTimeoutS":1200}"#).unwrap();
    let tasks = load_tasks(out.path());
    assert_eq!(tasks[0].oracle.trim(), "python3 -m pytest -q tests/x.py");
    assert_eq!(tasks[0].setup.as_deref(), Some("pip install -e ."));
    assert_eq!(tasks[0].oracle_timeout_s, 1200);
}

#[test]
fn swebench_adapter_builds_task_dirs_from_instances() {
    let root = tempfile::tempdir().unwrap();
    // a local stand-in for github.com/acme/lib
    let upstream = root.path().join("upstream/acme/lib");
    std::fs::create_dir_all(&upstream).unwrap();
    std::fs::write(upstream.join("mathx.py"), "def sum_range(a, b):\n    return sum(range(a, b))\n").unwrap();
    sh(&upstream, "git init -q -b main && git config user.email a@b && git config user.name t && git add -A && git commit -qm base");
    let base = String::from_utf8(Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&upstream).output().unwrap().stdout).unwrap().trim().to_string();
    let test_patch = "diff --git a/test_hidden.py b/test_hidden.py\nnew file mode 100644\n--- /dev/null\n+++ b/test_hidden.py\n@@ -0,0 +1,6 @@\n+import unittest\n+from mathx import sum_range\n+\n+class T(unittest.TestCase):\n+    def test_it(self):\n+        self.assertEqual(sum_range(1, 4), 10)\n";
    let inst = serde_json::json!({"instance_id": "acme__lib-1", "repo": "acme/lib", "base_commit": base, "problem_statement": "sum_range should be inclusive.", "test_patch": test_patch, "FAIL_TO_PASS": "[\"test_hidden.T.test_it\"]", "PASS_TO_PASS": [], "version": "1.0"});
    let skipped = serde_json::json!({"instance_id": "sympy__sympy-1", "repo": "sympy/sympy", "base_commit": base, "problem_statement": "x", "test_patch": "", "FAIL_TO_PASS": ["test_x"], "PASS_TO_PASS": [], "version": "1"});
    let file = root.path().join("instances.jsonl");
    std::fs::write(&file, format!("{}\n{}\n", inst, skipped)).unwrap();
    let out = root.path().join("tasks");
    let script = format!("{}/scripts/swebench_to_tasks.py", env!("CARGO_MANIFEST_DIR"));
    let r = Command::new("python3")
        .args([&script, file.to_str().unwrap(), "--out", out.to_str().unwrap(), "--cache", root.path().join("cache").to_str().unwrap(), "--clone-url-template", &format!("file://{}/upstream/{{repo}}", root.path().display()), "--pytest-cmd", "python3 -m unittest"])
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(String::from_utf8_lossy(&r.stderr).contains("sympy__sympy-1"), "the unsupported repo is reported as skipped");
    let t = out.join("acme__lib-1");
    assert!(t.join("repo/test_hidden.py").exists(), "hidden tests applied");
    assert!(!t.join("repo/.git").exists());
    assert!(!t.join("repo/oracle.sh").exists(), "the oracle stays outside repo/");
    let tasks = load_tasks(&out);
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].prompt.contains("inclusive"));
    // the oracle fails on the starting state and passes once the bug is fixed
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    sh(&work, &format!("cp -r {}/repo/. .", t.display()));
    let run_oracle = |dir: &Path| Command::new("sh").arg("-c").arg(&tasks[0].oracle).current_dir(dir).output().unwrap().status.success();
    assert!(!run_oracle(&work));
    std::fs::write(work.join("mathx.py"), "def sum_range(a, b):\n    return sum(range(a, b + 1))\n").unwrap();
    assert!(run_oracle(&work));
    let idx: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("INDEX.json")).unwrap()).unwrap();
    assert_eq!(idx["converted"][0], "acme__lib-1");
}

#[test]
fn polyglot_adapter_builds_verified_tasks_and_hides_the_reference_solution() {
    let root = tempfile::tempdir().unwrap();
    let ex = root.path().join("bench/python/exercises/practice/hello");
    std::fs::create_dir_all(ex.join(".docs")).unwrap();
    std::fs::create_dir_all(ex.join(".meta")).unwrap();
    std::fs::write(ex.join(".docs/instructions.md"), "# Instructions\n\nReturn the greeting `Hello, World!`.\n").unwrap();
    std::fs::write(ex.join(".meta/config.json"), r#"{"files":{"solution":["hello.py"],"test":["hello_test.py"],"example":[".meta/example.py"]}}"#).unwrap();
    std::fs::write(ex.join(".meta/example.py"), "def hello():\n    return 'Hello, World!'\n").unwrap();
    std::fs::write(ex.join("hello.py"), "def hello():\n    pass\n").unwrap();
    std::fs::write(ex.join("hello_test.py"), "import unittest\nfrom hello import hello\n\nclass T(unittest.TestCase):\n    def test_it(self):\n        self.assertEqual(hello(), 'Hello, World!')\n").unwrap();
    let out = root.path().join("out");
    let script = format!("{}/scripts/polyglot_to_tasks.py", env!("CARGO_MANIFEST_DIR"));
    let r = Command::new("python3").args([&script, "--repo", root.path().join("bench").to_str().unwrap(), "--langs", "python", "--out", out.to_str().unwrap(), "--verify"]).output().unwrap();
    assert!(r.status.success(), "{}{}", String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr));
    let t = out.join("polyglot-python-hello");
    assert!(t.join("repo/hello_test.py").exists() && !t.join("repo/.meta").exists() && !t.join("repo/.docs").exists(), "reference solution and docs must not be in repo/");
    let tasks = load_tasks(&out);
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].prompt.contains("Hello, World!") && tasks[0].prompt.contains("hello.py") && tasks[0].prompt.contains("Do not modify the test files"));
    assert!(tasks[0].tags.contains(&"polyglot".to_string()));
    // an exercise whose oracle already passes on the stub is dropped by --verify
    std::fs::write(ex.join("hello.py"), "def hello():\n    return 'Hello, World!'\n").unwrap();
    let out2 = root.path().join("out2");
    let r = Command::new("python3").args([&script, "--repo", root.path().join("bench").to_str().unwrap(), "--langs", "python", "--out", out2.to_str().unwrap(), "--verify"]).output().unwrap();
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("passes on the stub"));
}

#[test]
fn fanout_rule_and_cost_per_solved_task() {
    use fh::eval::runner::{fanout_rule, summarize, EvalRow, RunnerStats};
    let row = |runner: &str, solved: bool, secs: f64, tin: u64, cached: u64, tout: u64| EvalRow { id: "t".into(), runner: runner.into(), solved, seconds: secs, tokens_in: Some(tin), cached_tokens: Some(cached), tokens_out: Some(tout), ..Default::default() };
    // cached tokens are subtracted; when the server reports none, the measured prefix-cache hit rate is used
    assert_eq!(row("fh", true, 1.0, 10_000, 8_000, 500).cost_tokens(), Some(2_500));
    let mut est = row("fh", true, 1.0, 10_000, 0, 500);
    est.prefix_hit = Some(0.5);
    assert_eq!(est.cost_tokens(), Some(5_500));
    // fan-out: same pass rate, 30% faster, 1.5x the cost -> keep
    let fh: Vec<EvalRow> = (0..4).map(|_| row("fh", true, 7.0, 30_000, 24_000, 3_000)).collect();
    let single: Vec<EvalRow> = (0..4).map(|_| row("fh-single", true, 10.0, 20_000, 16_000, 2_000)).collect();
    let (a, b) = (RunnerStats::from_rows(&fh.iter().collect::<Vec<_>>()), RunnerStats::from_rows(&single.iter().collect::<Vec<_>>()));
    assert_eq!(a.cost_per_solved, Some(9_000.0));
    assert!(fanout_rule(&a, &b).iter().all(|r| r.1), "{:?}", fanout_rule(&a, &b));
    // too expensive (3x) and not faster -> the cost and time rules fail
    let fh2: Vec<EvalRow> = (0..4).map(|_| row("fh", true, 9.5, 60_000, 30_000, 6_000)).collect();
    let a2 = RunnerStats::from_rows(&fh2.iter().collect::<Vec<_>>());
    let r = fanout_rule(&a2, &b);
    assert_eq!(r.iter().map(|x| x.1).collect::<Vec<_>>(), vec![true, false, false], "{r:?}");
    // lower pass rate fails the first rule even when faster and cheaper
    let mut fh3 = fh.clone();
    fh3[0].solved = false;
    assert!(!fanout_rule(&RunnerStats::from_rows(&fh3.iter().collect::<Vec<_>>()), &b)[0].1);
    // the report prints the table and a verdict line
    let mut all = fh;
    all.extend(single);
    let md = summarize(&all);
    assert!(md.contains("uncached+completion tokens per solved task") && md.contains("Fan-out rule") && md.contains("=> keep fan-out on"), "{md}");
}
