use fh::agent::{run_agent, AgentOptions};
use fh::config::{load_config, Config, Env};
use fh::hooks::{HookEvent, Hooks};
use fh::llm::client::LlmClient;
use fh::testkit::{self, Scripted};
use fh::types::Mode;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

fn cfg(url: &str) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = url.into();
    c.retries = 0;
    c
}

fn env_with_home(home: &Path) -> Env {
    let mut e = Env::new();
    e.insert("FH_HOME".into(), home.to_string_lossy().to_string());
    e.insert("FH_CONFIG_HOME".into(), home.join("cfg").to_string_lossy().to_string());
    e
}

fn write_hooks(ws: &Path, v: serde_json::Value) {
    std::fs::create_dir_all(ws.join(".fh")).unwrap();
    std::fs::write(ws.join(".fh/hooks.json"), v.to_string()).unwrap();
}

#[tokio::test]
async fn allow_block_timeout_and_stdin_payload() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let env = env_with_home(home.path());
    write_hooks(ws.path(), json!({"hooks": {"PreToolUse": [
        {"matcher": "edit", "command": "cat > payload.json; echo 'no edits today' >&2; exit 2"},
        {"matcher": "bash", "command": "sleep 5", "timeout": 300},
        {"matcher": "read_file", "command": "exit 1"}
    ]}}));
    fh::trust::trust(&env, ws.path()).unwrap();
    let h = Hooks::load(ws.path(), &env);
    let blocked = h.fire(HookEvent::PreToolUse, json!({"tool_name": "edit", "tool_input": {"path": "a"}})).await;
    assert_eq!(blocked.blocked.as_deref(), Some("no edits today"));
    let payload: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(ws.path().join("payload.json")).unwrap()).unwrap();
    assert_eq!(payload["event"], "PreToolUse");
    assert_eq!(payload["tool_input"]["path"], "a");
    let t = h.fire(HookEvent::PreToolUse, json!({"tool_name": "bash"})).await;
    assert!(t.blocked.is_none() && t.notes[0].contains("timed out"), "{:?}", t.notes);
    let f = h.fire(HookEvent::PreToolUse, json!({"tool_name": "read_file"})).await;
    assert!(f.blocked.is_none() && f.notes[0].contains("exit 1"));
    let none = h.fire(HookEvent::PreToolUse, json!({"tool_name": "grep"})).await;
    assert!(none.blocked.is_none() && none.notes.is_empty());
}

#[tokio::test]
async fn untrusted_workspace_never_runs_project_hooks() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let env = env_with_home(home.path());
    write_hooks(ws.path(), json!({"hooks": {"PreToolUse": [{"command": "touch ran.txt; exit 2"}]}}));
    let h = Hooks::load(ws.path(), &env);
    assert!(h.is_empty() && h.skipped_untrusted);
    let o = h.fire(HookEvent::PreToolUse, json!({"tool_name": "edit"})).await;
    assert!(o.blocked.is_none());
    assert!(!ws.path().join("ran.txt").exists());
}

#[tokio::test]
async fn user_hooks_run_without_trust() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let env = env_with_home(home.path());
    std::fs::create_dir_all(home.path().join("cfg")).unwrap();
    std::fs::write(home.path().join("cfg/hooks.json"), json!({"hooks": {"UserPromptSubmit": [{"command": "echo 'careful' >&2; exit 2"}]}}).to_string()).unwrap();
    let h = Hooks::load(ws.path(), &env);
    let o = h.fire(HookEvent::UserPromptSubmit, json!({"prompt": "x"})).await;
    assert_eq!(o.blocked.as_deref(), Some("careful"));
}

#[tokio::test]
async fn pre_tool_hook_blocks_edit_in_agent_loop_and_post_hook_feeds_back() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let env = env_with_home(home.path());
    std::fs::write(ws.path().join("a.txt"), "x").unwrap();
    write_hooks(ws.path(), json!({"hooks": {
        "PreToolUse": [{"matcher": "edit", "command": "echo 'frozen file' >&2; exit 2"}],
        "PostToolUse": [{"matcher": "read_file", "command": "echo 'remember the style guide' >&2; exit 2"}]
    }}));
    fh::trust::trust(&env, ws.path()).unwrap();
    let m = testkit::start(0, None).await;
    m.push(Scripted::call("read_file", json!({"path": "a.txt"})));
    m.push(Scripted::call("edit", json!({"path": "a.txt", "old_text": "x", "new_text": "y"})));
    m.push(Scripted::text("done"));
    let mut o = AgentOptions::new(LlmClient::new(cfg(&m.url), Env::new()), ws.path(), Mode::AutoEdit);
    o.hooks = Some(Arc::new(Hooks::load(ws.path(), &env)));
    run_agent("t", o).await;
    assert_eq!(std::fs::read_to_string(ws.path().join("a.txt")).unwrap(), "x");
    let reqs = m.requests();
    let contents = |i: usize| reqs[i]["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(contents(1).contains("remember the style guide"), "{}", contents(1));
    assert!(contents(2).contains("frozen file"), "{}", contents(2));
}
