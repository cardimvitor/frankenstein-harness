use fh::config::{load_config, Config, Env};
use fh::direct::{apply_edits, build_context, parse_edits, run_direct, Edit};
use fh::testkit::{self, Mock, Scripted};
use serde_json::Value;
use std::path::Path;

fn cfg(m: &Mock) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = m.url.clone();
    c.retries = 0;
    c
}

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (k, v) in files {
        let p = d.path().join(k);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, v).unwrap();
    }
    d
}

const REPLY: &str = "Here is the fix.\n\n```python\nFile: mathx.py\n<<<<<<< SEARCH\n    for i in range(a, b):\n=======\n    for i in range(a, b + 1):\n>>>>>>> REPLACE\n```\n";

#[test]
fn parses_blocks_with_fences_prefixes_and_several_files() {
    let e = parse_edits(REPLY);
    assert_eq!(e, vec![Edit { path: "mathx.py".into(), search: "    for i in range(a, b):".into(), replace: "    for i in range(a, b + 1):".into() }]);
    let two = "`src/a.py`\n<<<<<<< SEARCH\nx = 1\n=======\nx = 2\n>>>>>>> REPLACE\n\n**src/b.py**\n<<<<<<< SEARCH\n=======\nnew\n>>>>>>> REPLACE\n";
    let e2 = parse_edits(two);
    assert_eq!(e2.len(), 2);
    assert_eq!((e2[0].path.as_str(), e2[1].path.as_str()), ("src/a.py", "src/b.py"));
    assert_eq!(e2[1].search, "");
    assert!(parse_edits("no blocks here").is_empty());
    assert!(parse_edits("a.py\n<<<<<<< SEARCH\nx\n=======\ny\n").is_empty(), "an unterminated block is ignored");
}

#[test]
fn applies_once_refuses_ambiguity_missing_text_and_workspace_escapes() {
    let d = repo(&[("a.py", "x = 1\ny = 1\nz = 1\n"), ("dup.py", "v = 1\nv = 1\n")]);
    let ed = |p: &str, s: &str, r: &str| Edit { path: p.into(), search: s.into(), replace: r.into() };
    let (changed, failed) = apply_edits(
        d.path(),
        &[ed("a.py", "y = 1", "y = 2"), ed("dup.py", "v = 1", "v = 9"), ed("a.py", "not there", "q"), ed("../escape.py", "", "boom"), ed("new/dir/f.py", "", "hello"), ed("a.py", "", "overwrite?")],
    );
    assert_eq!(std::fs::read_to_string(d.path().join("a.py")).unwrap(), "x = 1\ny = 2\nz = 1\n");
    assert_eq!(std::fs::read_to_string(d.path().join("new/dir/f.py")).unwrap(), "hello\n");
    assert_eq!(std::fs::read_to_string(d.path().join("dup.py")).unwrap(), "v = 1\nv = 1\n");
    assert!(!d.path().parent().unwrap().join("escape.py").exists());
    let why = |p: &str| failed.iter().find(|(f, _)| f == p).map(|(_, w)| w.clone()).unwrap_or_default();
    assert!(why("dup.py").contains("2 times"), "{failed:?}");
    assert!(why("../escape.py").contains("escapes"), "{failed:?}");
    assert_eq!(failed.iter().filter(|(f, _)| f == "a.py").count(), 2); // not found + empty SEARCH on an existing file
    assert_eq!(changed, vec!["a.py".to_string(), "new/dir/f.py".to_string()]);
}

#[test]
fn context_puts_docs_and_named_files_first_and_respects_the_budget() {
    let big = "z".repeat(5000);
    let d = repo(&[("README.md", "# readme"), ("src/target.py", "print('t')"), ("src/other.py", "print('o')"), ("src/big.py", &big)]);
    let (ctx, included, total) = build_context(d.path(), "fix src/target.py please", 200_000);
    assert_eq!((included, total), (4, 4));
    let pos = |s: &str| ctx.find(&format!("=== {s} ===")).unwrap_or(usize::MAX);
    assert!(pos("README.md") < pos("src/target.py") && pos("src/target.py") < pos("src/other.py"));
    assert!(ctx.contains("Repository files:\nREADME.md"));
    let (small, inc2, _) = build_context(d.path(), "fix src/target.py please", 300);
    assert!(inc2 < 4 && small.contains("=== README.md ==="));
    assert!(!small.contains("zzzzzz"));
}

#[tokio::test]
async fn direct_is_one_request_without_tools_and_applies_the_edit() {
    let m = testkit::start(0, None).await;
    m.push(Scripted::text(REPLY));
    let d = repo(&[("README.md", "Fix sum_range."), ("mathx.py", "def sum_range(a, b):\n    total = 0\n    for i in range(a, b):\n        total += i\n    return total\n")]);
    let r: Value = run_direct(&cfg(&m), &Env::new(), d.path(), "Fix the bug in mathx.py", false).await;
    assert_eq!(r["verdict"], "applied", "{r}");
    assert!(std::fs::read_to_string(d.path().join("mathx.py")).unwrap().contains("range(a, b + 1)"));
    assert_eq!(r["changed"][0], "mathx.py");
    assert_eq!(r["llm"]["requests"], 1);
    let reqs = m.requests();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].get("tools").is_none(), "no tools: the model alone");
    let msgs = reqs[0]["messages"].to_string();
    assert!(msgs.contains("=== mathx.py ===") && msgs.contains("Fix the bug in mathx.py") && msgs.contains("SEARCH/REPLACE"));
}

#[tokio::test]
async fn direct_reports_failure_for_a_reply_without_edits_and_error_when_the_model_is_unreachable() {
    let m = testkit::start(0, None).await;
    m.push(Scripted::text("I would change the loop bound."));
    let d = repo(&[("mathx.py", "x = 1\n")]);
    let r = run_direct(&cfg(&m), &Env::new(), d.path(), "fix", false).await;
    assert_eq!(r["verdict"], "failed");
    assert!(r["reason"].as_str().unwrap().contains("no SEARCH/REPLACE"));
    assert_eq!(std::fs::read_to_string(d.path().join("mathx.py")).unwrap(), "x = 1\n");
    // unreachable endpoint: an infrastructure error with zero requests (the adapters treat that as an error to retry)
    let mut c = cfg(&m);
    c.endpoint = "http://127.0.0.1:9/v1".into();
    let r2 = run_direct(&c, &Env::new(), d.path(), "fix", false).await;
    assert_eq!(r2["verdict"], "error");
    assert_eq!(r2["llm"]["requests"], 0, "{r2}");
}
