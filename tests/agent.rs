use fh::agent::{run_agent, AgentOptions, Stopped};
use fh::config::{load_config, Config, Env};
use fh::llm::client::LlmClient;
use fh::testkit::{self, Scripted};
use fh::tools::fs::*;
use fh::tools::{Tool, ToolCtx};
use fh::types::Mode;
use serde_json::json;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn cfg(url: &str) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = url.into();
    c.retries = 0;
    c
}

#[tokio::test]
async fn edit_unique_ambiguous_fuzzy_and_stale() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.txt");
    std::fs::write(&f, "one\n  two\nthree\none\n").unwrap();
    let ctx = ToolCtx::new(d.path());
    let edit = Edit::new();
    assert!(!edit.execute(&json!({"path":"a.txt","old_text":"one","new_text":"x"}), &ctx).await.ok); // ambiguous
    assert!(edit.execute(&json!({"path":"a.txt","old_text":"two","new_text":"2"}), &ctx).await.ok);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "one\n  2\nthree\none\n");
    assert!(edit.execute(&json!({"path":"a.txt","old_text":"three\n","new_text":"3\n"}), &ctx).await.ok);
    ReadFile::new().execute(&json!({"path":"a.txt"}), &ctx).await;
    std::fs::write(&f, "changed").unwrap();
    let r = edit.execute(&json!({"path":"a.txt","old_text":"changed","new_text":"y"}), &ctx).await;
    assert!(r.output.contains("changed since"), "{}", r.output);
    // fuzzy: indentation differs
    let d2 = tempfile::tempdir().unwrap();
    std::fs::write(d2.path().join("b.js"), "function f() {\n    return 1;\n}\n").unwrap();
    let ctx2 = ToolCtx::new(d2.path());
    assert!(edit.execute(&json!({"path":"b.js","old_text":"return 1;","new_text":"return 2;"}), &ctx2).await.ok);
    assert!(std::fs::read_to_string(d2.path().join("b.js")).unwrap().contains("return 2;"));
}

#[tokio::test]
async fn write_file_ownership_and_confinement() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("x.txt"), "hi").unwrap();
    std::fs::create_dir(d.path().join("src")).unwrap();
    let w = WriteNew::new();
    assert!(!w.execute(&json!({"path":"x.txt","content":"n"}), &ToolCtx::new(d.path())).await.ok);
    let mut owned = ToolCtx::new(d.path());
    owned.owned_globs = Some(vec!["src/**".into()]);
    assert!(w.execute(&json!({"path":"src/a.ts","content":"n"}), &owned).await.ok);
    let r = w.execute(&json!({"path":"lib/a.ts","content":"n"}), &owned).await;
    assert!(!r.ok && r.output.contains("ownership"), "{}", r.output);
    let r = w.execute(&json!({"path":"../evil","content":"n"}), &ToolCtx::new(d.path())).await;
    assert!(!r.ok && r.output.contains("escapes"));
    let r = w.execute(&json!({"path":".git/config","content":"n"}), &ToolCtx::new(d.path())).await;
    assert!(!r.ok);
}

#[tokio::test]
async fn bash_exit_timeout_cancel_and_env_scrub() {
    let d = tempfile::tempdir().unwrap();
    let bash = Bash::new();
    assert!(bash.execute(&json!({"command":"echo hi"}), &ToolCtx::new(d.path())).await.output.contains("hi"));
    std::env::set_var("MY_SECRET_TOKEN", "abcdefgh12345");
    let r = bash.execute(&json!({"command":"echo \"[$MY_SECRET_TOKEN]\""}), &ToolCtx::new(d.path())).await;
    assert!(!r.output.contains("abcdefgh"), "{}", r.output);
    let mut c = ToolCtx::new(d.path());
    c.bash_timeout_ms = 300;
    let t = bash.execute(&json!({"command":"sleep 5","timeout_s":1}), &c).await;
    assert!(t.output.contains("timed out"), "{}", t.output);
    let tok = CancellationToken::new();
    let t2 = tok.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(100)).await; t2.cancel(); });
    let mut c2 = ToolCtx::new(d.path());
    c2.cancel = Some(tok);
    let t0 = Instant::now();
    let r = bash.execute(&json!({"command":"sleep 5 & sleep 5; wait"}), &c2).await;
    assert!(r.output.contains("cancelled"), "{}", r.output);
    assert!(t0.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn agent_read_edit_final_with_repaired_call() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "hello world\n").unwrap();
    let m = testkit::start(0, None).await;
    m.push(Scripted::call("read_file", json!({"path":"a.txt"})));
    m.push(Scripted::call("edit", json!("{\"path\":\"a.txt\",\"old_text\":\"world\",\"new_text\":\"there\""))); // truncated JSON -> repaired
    m.push(Scripted::text("Changed greeting."));
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    let mut o = AgentOptions::new(LlmClient::new(cfg(&m.url), Env::new()), d.path(), Mode::AutoEdit);
    o.events.tool_end = Some(Arc::new(move |c, _, _| s2.lock().unwrap().push(c.name.clone())));
    let r = run_agent("change world to there", o).await;
    assert_eq!(r.stopped, Stopped::Done);
    assert_eq!(r.final_text, "Changed greeting.");
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "hello there\n");
    assert_eq!(*seen.lock().unwrap(), vec!["read_file", "edit"]);
    assert_eq!(r.touched, vec!["a.txt"]);
    let reqs = m.requests();
    assert_eq!(reqs[0]["chat_template_kwargs"]["enable_thinking"], true); // step 0 thinks
    assert_eq!(reqs[1]["chat_template_kwargs"]["enable_thinking"], false); // routine tool step does not
    assert_eq!(reqs[0]["messages"][0]["role"], "system");
}

#[tokio::test]
async fn agent_plan_mode_ask_mode_and_loop_detection() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "x").unwrap();
    let m = testkit::start(0, None).await;
    m.push(Scripted::call("edit", json!({"path":"a.txt","old_text":"x","new_text":"y"})));
    m.push(Scripted::text("done"));
    run_agent("t", AgentOptions::new(LlmClient::new(cfg(&m.url), Env::new()), d.path(), Mode::Plan)).await;
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "x");
    let last = m.requests()[1]["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(last.contains("plan mode"), "{last}");

    m.push(Scripted::call("edit", json!({"path":"a.txt","old_text":"x","new_text":"y"})));
    m.push(Scripted::text("done"));
    let asked = Arc::new(Mutex::new(0));
    let a2 = asked.clone();
    let mut o = AgentOptions::new(LlmClient::new(cfg(&m.url), Env::new()), d.path(), Mode::Ask);
    o.confirm = Some(Arc::new(move |_, _| { *a2.lock().unwrap() += 1; Box::pin(async { true }) }));
    run_agent("t", o).await;
    assert_eq!(*asked.lock().unwrap(), 1);
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "y");

    m.set_fallback(|_| Scripted::call("list_files", json!({})));
    let r = run_agent("t", AgentOptions::new(LlmClient::new(cfg(&m.url), Env::new()), d.path(), Mode::Yolo)).await;
    assert_eq!(r.stopped, Stopped::Loop);
}

#[tokio::test]
async fn agent_abort_mid_request() {
    let d = tempfile::tempdir().unwrap();
    let m = testkit::start(0, None).await;
    m.push(Scripted { delay_ms: 3000, content: Some("x".into()), ..Default::default() });
    let tok = CancellationToken::new();
    let t2 = tok.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(60)).await; t2.cancel(); });
    let mut o = AgentOptions::new(LlmClient::new(cfg(&m.url), Env::new()), d.path(), Mode::Yolo);
    o.cancel = Some(tok);
    let r = run_agent("t", o).await;
    assert_eq!(r.stopped, Stopped::Aborted);
}

#[tokio::test]
async fn llm_compaction_keeps_an_early_fact_that_pruning_would_lose() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), format!("the launch code is ZX-42\n{}", "padding ".repeat(400))).unwrap();
    std::fs::write(d.path().join("big.txt"), "filler line\n".repeat(2000)).unwrap();
    let run = |compaction: bool| {
        let d = d.path().to_path_buf();
        async move {
            let m = testkit::start(0, None).await;
            let step = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            m.set_fallback(move |req| {
                let msgs = req["messages"].as_array().unwrap();
                let sys = msgs[0]["content"].as_str().unwrap_or("");
                let all: String = msgs.iter().map(|x| x["content"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join("\n");
                if sys.contains("compress the working history") {
                    // a faithful summary of what it was shown
                    return Scripted::text(if all.contains("ZX-42") { "Read a.txt: it states the launch code is ZX-42." } else { "nothing notable" });
                }
                let tools = step.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if tools == 0 {
                    return Scripted::call("read_file", json!({"path": "a.txt"}));
                }
                if tools < 14 {
                    return Scripted::call("read_file", json!({"path": "big.txt", "offset": tools * 100, "limit": 90}));
                }
                Scripted::text(if all.contains("ZX-42") { "code ZX-42" } else { "code unknown" })
            });
            let mut o = AgentOptions::new(LlmClient::new(cfg(&m.url), Env::new()), &d, Mode::Yolo);
            o.context_window = 3000;
            o.llm_compaction = compaction;
            o.max_steps = 30;
            let r = run_agent("what is the launch code? read a.txt first", o).await;
            (r.final_text, m.requests().iter().filter(|q| q["messages"][0]["content"].as_str().unwrap_or("").contains("compress the working history")).count())
        }
    };
    let (with, summaries) = run(true).await;
    assert_eq!(with, "code ZX-42");
    assert!(summaries >= 1);
    let (without, none) = run(false).await;
    assert_eq!(without, "code unknown", "deterministic pruning alone loses the early fact");
    assert_eq!(none, 0);
}

#[tokio::test]
async fn multi_edit_is_atomic_and_syntax_errors_are_reported_in_the_same_turn() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("m.py"), "def a():\n    return 1\n\n\ndef b():\n    return 2\n").unwrap();
    let ctx = ToolCtx::new(d.path());
    let edit = Edit::new();
    // two changes in one call
    let r = edit.execute(&json!({"path": "m.py", "edits": [{"old_text": "return 1", "new_text": "return 10"}, {"old_text": "return 2", "new_text": "return 20"}]}), &ctx).await;
    assert!(r.ok && r.output.contains("2 changes") && !r.output.contains("syntax"), "{}", r.output);
    assert_eq!(std::fs::read_to_string(d.path().join("m.py")).unwrap(), "def a():\n    return 10\n\n\ndef b():\n    return 20\n");
    // all or nothing: the second edit does not match, so the first is not applied either
    let before = std::fs::read_to_string(d.path().join("m.py")).unwrap();
    let r = edit.execute(&json!({"path": "m.py", "edits": [{"old_text": "return 10", "new_text": "return 11"}, {"old_text": "no such text", "new_text": "x"}]}), &ctx).await;
    assert!(!r.ok && r.output.contains("edit 2 of 2") && r.output.contains("Nothing was changed"), "{}", r.output);
    assert_eq!(std::fs::read_to_string(d.path().join("m.py")).unwrap(), before);
    // edits later in the batch see the result of earlier ones
    let r = edit.execute(&json!({"path": "m.py", "edits": [{"old_text": "return 10", "new_text": "return 11"}, {"old_text": "return 11", "new_text": "return 12"}]}), &ctx).await;
    assert!(r.ok, "{}", r.output);
    assert!(std::fs::read_to_string(d.path().join("m.py")).unwrap().contains("return 12"));
    // a broken edit is flagged right away, but the edit is applied (the model fixes it next turn)
    let r = edit.execute(&json!({"path": "m.py", "old_text": "def a():", "new_text": "def a(:"}), &ctx).await;
    assert!(r.ok && r.output.contains("syntax check failed") && r.output.to_lowercase().contains("syntaxerror"), "{}", r.output);
    // new files are checked too; JSON in-process
    let w = WriteNew::new();
    let r = w.execute(&json!({"path": "c.json", "content": "{\"a\": 1,}"}), &ctx).await;
    assert!(r.ok && r.output.contains("invalid JSON"), "{}", r.output);
    let r = w.execute(&json!({"path": "ok.json", "content": "{\"a\": 1}"}), &ctx).await;
    assert!(r.ok && !r.output.contains("syntax"), "{}", r.output);
    // file names with quotes and spaces do not break the checker
    let r = w.execute(&json!({"path": "it's fine.py", "content": "x = 1\n"}), &ctx).await;
    assert!(r.ok && !r.output.contains("syntax"), "{}", r.output);
    // single-edit form still works and reports nothing extra for unknown file types
    std::fs::write(d.path().join("n.txt"), "hello").unwrap();
    let r = edit.execute(&json!({"path": "n.txt", "old_text": "hello", "new_text": "bye"}), &ctx).await;
    assert_eq!(r.output, "edited n.txt");
}
