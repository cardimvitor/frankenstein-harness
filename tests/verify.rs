use fh::config::{load_config, Config, Env};
use fh::fingerprint::{fingerprint, similarity};
use fh::llm::client::LlmClient;
use fh::session::checkpoint::Checkpoints;
use fh::testkit::{self, Scripted};
use fh::verify::checks::{diff_checks, parse_diff_added, Status};
use fh::verify::reviewer::validate_findings;
use fh::verify::rounds::{verify_loop, Verdict, VerifyCtx};
use fh::verify::secrets::find_secrets;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (k, v) in files {
        let p = d.path().join(k);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, v).unwrap();
    }
    let ok = Command::new("sh").arg("-c").arg("git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm init").current_dir(d.path()).status().unwrap();
    assert!(ok.success());
    d
}
fn cfg(url: &str) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = url.into();
    c.retries = 0;
    c
}

#[tokio::test]
async fn checkpoint_snapshot_diff_restore_leaves_user_index_alone() {
    let d = repo(&[("a.txt", "one"), ("b.txt", "two")]);
    let cp = Checkpoints::new(d.path());
    let id = cp.create("t").await.unwrap();
    std::fs::write(d.path().join("a.txt"), "changed").unwrap();
    std::fs::remove_file(d.path().join("b.txt")).unwrap();
    std::fs::write(d.path().join("new.txt"), "n").unwrap();
    let ch = cp.changed_since(&id).await;
    assert_eq!((ch.added.clone(), ch.modified.clone(), ch.deleted.clone()), (vec!["new.txt".to_string()], vec!["a.txt".to_string()], vec!["b.txt".to_string()]));
    assert!(cp.diff_since(&id).await.contains("+changed"));
    cp.restore(&id).await;
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "one");
    assert_eq!(std::fs::read_to_string(d.path().join("b.txt")).unwrap(), "two");
    assert!(!d.path().join("new.txt").exists());
    let st = Command::new("git").args(["status", "--porcelain"]).current_dir(d.path()).output().unwrap();
    assert!(String::from_utf8_lossy(&st.stdout).trim().is_empty());
}

#[tokio::test]
async fn diff_checks_scope_secrets_tests_and_markers() {
    let d = repo(&[("src/a.ts", "x"), ("test/a.test.ts", "it('a', ()=>{})")]);
    let cp = Checkpoints::new(d.path());
    let id = cp.create("t").await.unwrap();
    std::fs::write(d.path().join("src/a.ts"), "const k = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\";\n<<<<<<< HEAD\n").unwrap();
    std::fs::remove_file(d.path().join("test/a.test.ts")).unwrap();
    std::fs::write(d.path().join("test/b.test.ts"), "it.skip('b', ()=>{})").unwrap();
    std::fs::write(d.path().join("other.txt"), "o").unwrap();
    let r = diff_checks(&cp, &id, Some(&["src/**".to_string()])).await;
    let by = |n: &str| r.results.iter().find(|c| c.name == n).unwrap().clone();
    assert_eq!(by("diff scope").status, Status::Fail);
    assert!(by("diff scope").detail.contains("other.txt"));
    assert_eq!(by("secret scan").status, Status::Fail);
    assert_eq!(by("test integrity").status, Status::Fail);
    assert!(by("test integrity").detail.contains("deleted test file") && by("test integrity").detail.contains("skipped"));
    assert_eq!(by("patch sanity").status, Status::Fail);
    let lines: Vec<usize> = parse_diff_added("+++ b/x\n@@ -1 +5,2 @@\n+a\n b\n+c").iter().map(|a| a.line).collect();
    assert_eq!(lines, vec![5, 7]);
}

#[test]
fn uncheckable_findings_are_dropped() {
    let d = repo(&[("a.ts", "line1\nconst total = a + b;\nline3\n")]);
    let raw = json!([
        {"file":"a.ts","line":2,"severity":"blocker","claim":"bad","quote":"const total = a + b;"},
        {"file":"a.ts","line":2,"severity":"blocker","claim":"hallucinated quote","quote":"something else entirely"},
        {"file":"a.ts","line":99,"severity":"major","claim":"oob","quote":"x y z"},
        {"file":"nope.ts","line":1,"severity":"major","claim":"unknown file","quote":"line1"},
        {"file":"a.ts","severity":"major","claim":"no line"}
    ]);
    let (valid, dropped) = validate_findings(d.path(), &raw, &["a.ts".to_string()]);
    assert_eq!((valid.len(), dropped), (1, 4));
}

#[test]
fn secrets_detected() {
    assert!(!find_secrets("token = \"abcdefghijklmnopqrstuvwx1234\"").is_empty());
    assert!(find_secrets("const a = 1").is_empty());
}

#[test]
fn fingerprint_detects_stacks_and_verify_commands() {
    let a = repo(&[("App/App.csproj", "<Project><PropertyGroup><TargetFrameworkVersion>v4.8</TargetFrameworkVersion></PropertyGroup></Project>"), ("App.sln", "")]);
    let ids: Vec<String> = fingerprint(a.path()).stacks.iter().map(|s| format!("{}@{}", s.id, s.version.clone().unwrap_or_default())).collect();
    assert_eq!(ids, vec!["dotnet-framework@48"]);
    let b = repo(&[("x/x.csproj", "<Project><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup></Project>"), ("package.json", r#"{"dependencies":{"react":"^18.2.0"},"scripts":{"test":"vitest","build":"vite build"}}"#)]);
    let fb = fingerprint(b.path());
    assert!(fb.stacks.iter().any(|s| s.id == "dotnet" && s.version.as_deref() == Some("8")));
    assert!(fb.stacks.iter().any(|s| s.id == "react" && s.version.as_deref() == Some("18")));
    assert!(fb.verify.iter().any(|v| v.name == "dotnet build") && fb.verify.iter().any(|v| v.name == "build"));
    let c = repo(&[("package.json", r#"{"dependencies":{"angular":"1.8.2"}}"#)]);
    assert!(fingerprint(c.path()).stacks.iter().any(|s| s.id == "angularjs"));
    let e = repo(&[("package.json", r#"{"dependencies":{"@angular/core":"^17.1.0"}}"#)]);
    assert!(fingerprint(e.path()).stacks.iter().any(|s| s.id == "angular" && s.version.as_deref() == Some("17")));
    assert_eq!(similarity(&fb.tokens, &fingerprint(b.path()).tokens), 1.0);
    assert!(similarity(&fb.tokens, &fingerprint(c.path()).tokens) < 0.5);
}

#[tokio::test]
async fn verify_loop_fail_fix_pass_and_unverified() {
    let d = repo(&[("package.json", r#"{"scripts":{"test":"node check.js"}}"#), ("check.js", "process.exit(require('fs').readFileSync('v.txt','utf8').trim()==='ok'?0:1)"), ("v.txt", "bad")]);
    let cp = Checkpoints::new(d.path());
    let base = cp.create("t").await.unwrap();
    std::fs::write(d.path().join("v.txt"), "still bad").unwrap();
    let fp = fingerprint(d.path());
    let fixes = Arc::new(Mutex::new(0));
    let (f2, dp) = (fixes.clone(), d.path().to_path_buf());
    let ctx = VerifyCtx { cwd: d.path(), cp: &cp, base: &base, fp: &fp, llm: None, acceptance: &[], allowed_globs: None, max_rounds: 3, cancel: None };
    let rep = verify_loop(&ctx, move |fb, _| {
        *f2.lock().unwrap() += 1;
        assert!(fb.contains("CHECK FAILED: test"));
        std::fs::write(dp.join("v.txt"), "ok").unwrap();
        async {}
    })
    .await;
    assert_eq!(rep.verdict, Verdict::Pass);
    assert_eq!(rep.rounds.len(), 2);
    assert_eq!(*fixes.lock().unwrap(), 1);

    let d2 = repo(&[("a.txt", "x")]);
    let cp2 = Checkpoints::new(d2.path());
    let b2 = cp2.create("t").await.unwrap();
    std::fs::write(d2.path().join("a.txt"), "y").unwrap();
    let fp2 = fingerprint(d2.path());
    let ctx2 = VerifyCtx { cwd: d2.path(), cp: &cp2, base: &b2, fp: &fp2, llm: None, acceptance: &[], allowed_globs: None, max_rounds: 1, cancel: None };
    assert_eq!(verify_loop(&ctx2, |_, _| async {}).await.verdict, Verdict::Unverified);
}

#[tokio::test]
async fn reviewer_blocker_with_valid_citation_forces_a_fix_round() {
    let d = repo(&[("package.json", r#"{"scripts":{"test":"node -e 0"}}"#), ("a.js", "const x = 1;\n")]);
    let cp = Checkpoints::new(d.path());
    let base = cp.create("t").await.unwrap();
    std::fs::write(d.path().join("a.js"), "const x = 1;\nconst y = x / 0;\n").unwrap();
    let m = testkit::start(0, None).await;
    m.push(Scripted::json(json!({"verdict":"fail","findings":[
        {"file":"a.js","line":2,"severity":"blocker","claim":"division by zero","quote":"const y = x / 0;"},
        {"file":"a.js","line":1,"severity":"blocker","claim":"invented","quote":"nonexistent text"}]})));
    m.push(Scripted::json(json!({"verdict":"pass","findings":[]})));
    let llm = LlmClient::new(cfg(&m.url), Env::new());
    let fp = fingerprint(d.path());
    let ctx = VerifyCtx { cwd: d.path(), cp: &cp, base: &base, fp: &fp, llm: Some(&llm), acceptance: &["no crashes".to_string()], allowed_globs: None, max_rounds: 3, cancel: None };
    let fb = Arc::new(Mutex::new(String::new()));
    let (fb2, dp): (_, PathBuf) = (fb.clone(), d.path().to_path_buf());
    let rep = verify_loop(&ctx, move |f, _| {
        *fb2.lock().unwrap() = f;
        std::fs::write(dp.join("a.js"), "const x = 1;\nconst y = x / 2;\n").unwrap();
        async {}
    })
    .await;
    let f = fb.lock().unwrap().clone();
    assert!(f.contains("a.js:2") && !f.contains("invented"), "{f}");
    assert_eq!(rep.verdict, Verdict::Pass);
    assert_eq!((rep.reviewer.dropped, rep.reviewer.valid), (1, 1));
    assert_eq!(m.requests()[0]["response_format"]["type"], "json_schema");
}
