use fh::config::{load_config, Config, Env};
use fh::engine::{Engine, NoticeKind, TaskOptions};
use fh::eval::corpus::builtin_tasks;
use fh::skills::reuse::{OfferedSkill, ReuseOffer};
use fh::skills::store::SkillStore;
use fh::testkit::{self, Scripted};
use fh::tui::app::{Action, App, Item, Modal};
use fh::tui::io::{Ask, TuiIo, UiEvent};
use fh::types::Mode;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc::unbounded_channel, oneshot};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}
fn app() -> App {
    App::new("qwen-mock", "http://x/v1", "/w", Mode::AutoEdit, false)
}

#[test]
fn composer_keys_submit_newline_modes_and_cancel() {
    let mut a = app();
    type_str(&mut a, "fix it");
    assert_eq!(a.input, "fix it");
    a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    type_str(&mut a, "now");
    assert_eq!(a.input, "fix it\nnow");
    a.on_key(key(KeyCode::Backspace));
    assert_eq!(a.input, "fix it\nno");
    assert_eq!(a.on_key(key(KeyCode::Enter)), Action::Submit("fix it\nno".into()));
    assert!(a.busy && a.input.is_empty() && matches!(a.items.last(), Some(Item::User(_))));
    // while busy: a second Enter does not submit; Esc and Ctrl+C cancel
    type_str(&mut a, "more");
    assert_eq!(a.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(a.on_key(key(KeyCode::Esc)), Action::Cancel);
    assert_eq!(a.on_key(ctrl('c')), Action::Cancel);
    a.busy = false;
    assert_eq!(a.on_key(ctrl('c')), Action::Quit);
    // approval cycle and autonomous toggle
    let mut b = app();
    assert_eq!(b.approval, Mode::AutoEdit);
    b.on_key(key(KeyCode::Tab));
    assert_eq!(b.approval, Mode::Yolo);
    b.on_key(key(KeyCode::Tab));
    assert_eq!(b.approval, Mode::Plan);
    b.on_key(ctrl('a'));
    assert!(b.auto);
    b.on_key(ctrl('t'));
    assert!(b.show_reasoning);
    type_str(&mut b, "/exit");
    assert_eq!(b.on_key(key(KeyCode::Enter)), Action::Quit);
    let mut c = app();
    assert_eq!(c.on_key(ctrl('d')), Action::Quit);
    type_str(&mut c, "x");
    assert_eq!(c.on_key(ctrl('d')), Action::None);
}

#[test]
fn plan_modal_approve_reject_and_feedback() {
    for (keys, ok, fb) in [(vec![key(KeyCode::Enter)], true, None), (vec![key(KeyCode::Char('n'))], false, None), (vec![key(KeyCode::Char('e')), key(KeyCode::Char('h')), key(KeyCode::Char('i')), key(KeyCode::Enter)], false, Some("hi"))] {
        let mut a = app();
        let (tx, mut rx) = oneshot::channel();
        a.apply(UiEvent::Ask(Ask::Plan { plan: "1. do".into(), trivial: false, reply: tx }));
        assert!(a.has_modal());
        for k in keys {
            a.on_key(k);
        }
        let d = rx.try_recv().unwrap();
        assert_eq!((d.ok, d.feedback.as_deref()), (ok, fb));
        assert!(!a.has_modal());
    }
}

#[test]
fn questions_confirm_and_reuse_modals() {
    let mut a = app();
    let (tx, mut rx) = oneshot::channel();
    a.apply(UiEvent::Ask(Ask::Questions { questions: vec!["q1".into(), "q2".into()], reply: tx }));
    type_str(&mut a, "yes");
    a.on_key(key(KeyCode::Enter));
    assert!(a.has_modal());
    a.on_key(key(KeyCode::Enter)); // empty answer -> decide for me
    assert_eq!(rx.try_recv().unwrap(), vec!["yes".to_string(), "(use your best judgement)".to_string()]);

    let (tx, mut rx) = oneshot::channel();
    a.apply(UiEvent::Ask(Ask::Confirm { tool: "bash".into(), args: "{}".into(), reply: tx }));
    a.on_key(key(KeyCode::Char('x'))); // ignored
    assert!(a.has_modal());
    a.on_key(key(KeyCode::Char('y')));
    assert!(rx.try_recv().unwrap());

    let (tx, mut rx) = oneshot::channel();
    a.apply(UiEvent::Ask(Ask::Confirm { tool: "bash".into(), args: "{}".into(), reply: tx }));
    a.on_key(ctrl('c')); // cancel rejects the pending approval
    assert!(!rx.try_recv().unwrap());

    let offers = vec![ReuseOffer { from_project: "p1".into(), from_label: "ProjectX".into(), similarity: 1.0, skills: vec![OfferedSkill { id: "u:1".into(), name: "A".into(), summary: "s".into() }, OfferedSkill { id: "u:2".into(), name: "B".into(), summary: "s".into() }] }];
    let (tx, mut rx) = oneshot::channel();
    a.apply(UiEvent::Ask(Ask::Reuse { offers: offers.clone(), reply: tx }));
    a.on_key(key(KeyCode::Down));
    a.on_key(key(KeyCode::Char(' ')));
    a.on_key(key(KeyCode::Enter));
    assert_eq!(rx.try_recv().unwrap(), vec![("p1".to_string(), vec!["u:2".to_string()])]);
    let (tx, mut rx) = oneshot::channel();
    a.apply(UiEvent::Ask(Ask::Reuse { offers, reply: tx }));
    a.on_key(key(KeyCode::Char('n')));
    assert!(rx.try_recv().unwrap().is_empty());
    a.on_key(key(KeyCode::F(1)));
    assert!(matches!(a.modal, Modal::Help));
    a.on_key(key(KeyCode::Char('q')));
    assert!(!a.has_modal());
}

fn cfg_for(m: &testkit::Mock) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = m.url.clone();
    c.metrics_url = format!("{}/metrics", m.base);
    c.retries = 0;
    c
}

/// A whole guided session through the TUI's event pipeline: plan card appears, the "user" approves with Enter,
/// progress is streamed, and the verified result replaces the streamed text.
#[tokio::test]
async fn full_guided_session_through_the_tui_pipeline() {
    let m = testkit::start(0, None).await;
    m.set_fallback(|req| {
        let p = &req["response_format"]["json_schema"]["schema"]["properties"];
        if p.get("trivial").is_some() {
            return Scripted::json(json!({"trivial": false, "questions": [], "enriched": "fix sum_range", "acceptance": ["tests pass"], "plan": [{"step": "widen the range", "files": ["mathx.py"]}], "assumptions": [], "subtasks": []}));
        }
        if p.get("verdict").is_some() {
            return Scripted::json(json!({"verdict": "pass", "findings": []}));
        }
        if req["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" {
            return Scripted::text("Widened the range.");
        }
        Scripted { reasoning: Some("the loop stops early".into()), tool_calls: vec![("edit".into(), json!({"path": "mathx.py", "old_text": "in range(a, b)", "new_text": "in range(a, b + 1)"}))], ..Default::default() }
    });
    let t = builtin_tasks().into_iter().find(|t| t.id == "py-off-by-one").unwrap();
    let d = tempfile::tempdir().unwrap();
    for (f, b) in &t.files {
        std::fs::write(d.path().join(f), b).unwrap();
    }
    assert!(Command::new("sh").arg("-c").arg("git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm b").current_dir(d.path()).status().unwrap().success());

    let (tx, mut rx) = unbounded_channel::<UiEvent>();
    let engine = Arc::new(Engine::new(cfg_for(&m), Env::new(), Arc::new(TuiIo { tx: tx.clone() }), d.path(), SkillStore::in_memory()));
    let mut a = app();
    let e2 = engine.clone();
    let run = tokio::spawn(async move { e2.run_task("fix sum_range", TaskOptions { approval: Mode::AutoEdit, no_mine: true, ..Default::default() }).await });
    let (mut saw_plan, mut saw_tool) = (false, false);
    let mut run = run;
    let result = loop {
        tokio::select! {
            Some(ev) = rx.recv() => {
                a.apply(ev);
                if a.has_modal() && !saw_plan {
                    saw_plan = matches!(a.modal, Modal::Plan { .. });
                    a.on_key(key(KeyCode::Enter)); // the user approves the plan
                }
                saw_tool |= a.items.iter().any(|i| matches!(i, Item::Tool { .. }));
            }
            r = &mut run => break r.unwrap(),
            _ = tokio::time::sleep(Duration::from_secs(30)) => panic!("session timed out"),
        }
    };
    while let Ok(ev) = rx.try_recv() {
        a.apply(ev); // events emitted just before the engine returned
        saw_tool |= a.items.iter().any(|i| matches!(i, Item::Tool { .. }));
    }
    a.apply(UiEvent::Result(Box::new(result)));
    let (saw_progress_or_tool, result_seen) = (saw_tool, true);
    assert!(saw_plan && saw_progress_or_tool && result_seen);
    assert_eq!(a.last_verdict.as_deref(), Some("pass"));
    assert!(a.items.iter().any(|i| matches!(i, Item::Final { verdict, text, .. } if verdict == "pass" && text.starts_with("Verified"))));
    assert!(!a.items.iter().any(|i| matches!(i, Item::Progress(_))), "streamed text is replaced by the gated verdict");
    assert!(a.files.contains(&"mathx.py".to_string()));
    assert!(a.verify.iter().any(|(t, s)| t.contains("unittest") && s == "pass"), "{:?}", a.verify);
    assert!(std::fs::read_to_string(d.path().join("mathx.py")).unwrap().contains("b + 1"));
}

#[test]
fn resume_and_sessions_commands() {
    let mut a = app();
    type_str(&mut a, "/sessions");
    assert_eq!(a.on_key(key(KeyCode::Enter)), Action::Sessions);
    assert!(!a.busy && a.input.is_empty());
    type_str(&mut a, "/resume");
    assert_eq!(a.on_key(key(KeyCode::Enter)), Action::Resume(None));
    assert!(a.busy && matches!(a.items.last(), Some(Item::User(t)) if t == "/resume"));
    let mut b = app();
    type_str(&mut b, "/resume 0001700000000");
    assert_eq!(b.on_key(key(KeyCode::Enter)), Action::Resume(Some("0001700000000".into())));
    // while a task runs, neither starts a second one
    let mut c = app();
    c.busy = true;
    type_str(&mut c, "/resume");
    assert_eq!(c.on_key(key(KeyCode::Enter)), Action::None);
}

#[test]
fn search_command_in_the_tui() {
    let mut a = app();
    type_str(&mut a, "/search pagination orders");
    assert_eq!(a.on_key(key(KeyCode::Enter)), Action::Search("pagination orders".into()));
    assert!(!a.busy && a.input.is_empty());
}
