use super::app::{Action, App};
use super::io::{TuiIo, UiEvent};
use super::ui::draw;
use crate::config::{Config, Env};
use crate::engine::{Engine, TaskOptions};
use crate::skills::store::SkillStore;
use crate::types::Mode;
use ratatui::crossterm::event::{self, Event};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;

pub struct TuiOptions {
    pub approval: Mode,
    pub auto: bool,
    pub sandbox: bool,
    pub commit: bool,
}

/// Run the full-screen UI until the user quits. Returns the process exit code.
pub async fn run_tui(cfg: Config, env: Env, cwd: PathBuf, store: SkillStore, o: TuiOptions) -> i32 {
    let (tx, mut rx) = unbounded_channel::<UiEvent>();
    let io = Arc::new(TuiIo { tx: tx.clone() });
    let engine = Arc::new(Engine::new(cfg.clone(), env, io, cwd.clone(), store));
    let mut app = App::new(&cfg.model, &cfg.endpoint, &cwd.to_string_lossy(), o.approval, o.auto);
    app.items.push(super::app::Item::Notice(crate::engine::NoticeKind::Info, format!("{} @ {} — F1 for keys", cfg.model, cfg.endpoint)));

    // terminal input runs on its own thread so the async runtime never blocks on it
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (txi, stop) = (tx.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                    match event::read() {
                        Ok(Event::Key(k)) => {
                            let _ = txi.send(UiEvent::Key(k));
                        }
                        Ok(Event::Resize(..)) => {
                            let _ = txi.send(UiEvent::Resize);
                        }
                        _ => {}
                    }
                }
            }
        });
        let tick = tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(120)).await;
                if tick.send(UiEvent::Tick).is_err() {
                    break;
                }
            }
        });
    }

    let mut terminal = ratatui::init();
    let cancel: Arc<Mutex<Option<CancellationToken>>> = Arc::new(Mutex::new(None));
    let mut code = 0;
    loop {
        if terminal.draw(|f| draw(f, &mut app)).is_err() {
            code = 1;
            break;
        }
        let Some(ev) = rx.recv().await else { break };
        let action = match ev {
            UiEvent::Key(k) => app.on_key(k),
            other => {
                app.apply(other);
                Action::None
            }
        };
        match action {
            Action::None => {}
            Action::Quit => break,
            Action::Cancel => {
                if let Some(t) = cancel.lock().unwrap().as_ref() {
                    t.cancel();
                }
            }
            Action::Submit(text) => {
                let token = CancellationToken::new();
                *cancel.lock().unwrap() = Some(token.clone());
                let (engine, tx2) = (engine.clone(), tx.clone());
                let opts = TaskOptions { auto: app.auto, approval: app.approval, sandbox: o.sandbox, commit: o.commit, cancel: Some(token), ..Default::default() };
                tokio::spawn(async move {
                    let r = engine.run_task(&text, opts).await;
                    let _ = tx2.send(UiEvent::Result(Box::new(r)));
                    engine.drain(20_000).await;
                });
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    ratatui::restore();
    code
}
