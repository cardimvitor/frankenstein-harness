//! Lifecycle hooks: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop.
//! A hook is a shell command that receives the event as JSON on stdin. Exit code 2 blocks
//! (stderr is the reason); any other non-zero exit or a timeout is reported but never blocks.
//! User hooks always run; project hooks (`.fh/hooks.json`) only in a trusted workspace.
use crate::config::{config_dir, Env};
use crate::util::proc::{run, RunOpts};
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
}

impl HookEvent {
    pub fn name(&self) -> &'static str {
        match self {
            HookEvent::SessionStart => "SessionStart",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::Stop => "Stop",
        }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    event: String,
    matcher: Option<Regex>,
    command: String,
    timeout_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct HookOutcome {
    /// set when a hook exited with code 2
    pub blocked: Option<String>,
    /// non-blocking problems and hook stdout worth showing
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Hooks {
    entries: Vec<Entry>,
    cwd: PathBuf,
    /// project hook file exists but the workspace is not trusted
    pub skipped_untrusted: bool,
}

fn parse(v: &Value, out: &mut Vec<Entry>) {
    let Some(h) = v.get("hooks").and_then(|x| x.as_object()) else { return };
    for (event, list) in h {
        for item in list.as_array().cloned().unwrap_or_default() {
            let Some(cmd) = item.get("command").and_then(|c| c.as_str()) else { continue };
            let matcher = item.get("matcher").and_then(|m| m.as_str()).and_then(|m| Regex::new(&format!("^(?:{m})$")).ok());
            let timeout_ms = item.get("timeout").and_then(|t| t.as_u64()).unwrap_or(10_000);
            out.push(Entry { event: event.clone(), matcher, command: cmd.to_string(), timeout_ms });
        }
    }
}

fn read(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

impl Hooks {
    pub fn none() -> Self {
        Hooks::default()
    }

    pub fn load(cwd: &Path, env: &Env) -> Hooks {
        let mut entries = Vec::new();
        if let Some(v) = read(&config_dir(env).join("hooks.json")) {
            parse(&v, &mut entries);
        }
        let mut skipped = false;
        let project = cwd.join(".fh").join("hooks.json");
        if project.exists() {
            if crate::trust::is_trusted(env, cwd) {
                if let Some(v) = read(&project) {
                    parse(&v, &mut entries);
                }
            } else {
                skipped = true;
            }
        }
        Hooks { entries, cwd: cwd.to_path_buf(), skipped_untrusted: skipped }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Runs every matching hook in order; stops at the first block.
    pub async fn fire(&self, ev: HookEvent, mut payload: Value) -> HookOutcome {
        let mut out = HookOutcome::default();
        let tool = payload.get("tool_name").and_then(|t| t.as_str()).unwrap_or("").to_string();
        if let Some(o) = payload.as_object_mut() {
            o.insert("event".into(), json!(ev.name()));
            o.insert("cwd".into(), json!(self.cwd.to_string_lossy()));
        }
        let input = payload.to_string();
        for e in self.entries.iter().filter(|e| e.event == ev.name()) {
            if let Some(m) = &e.matcher {
                if !m.is_match(&tool) {
                    continue;
                }
            }
            let r = run(&e.command, &self.cwd, RunOpts { timeout: Some(Duration::from_millis(e.timeout_ms)), stdin: Some(input.clone()), ..Default::default() }).await;
            if r.timed_out {
                out.notes.push(format!("hook timed out after {} ms: {}", e.timeout_ms, e.command));
                continue;
            }
            match r.code {
                Some(0) => {
                    let s = r.stdout.trim();
                    if !s.is_empty() {
                        out.notes.push(s.to_string());
                    }
                }
                Some(2) => {
                    let why = r.stderr.trim();
                    out.blocked = Some(if why.is_empty() { format!("blocked by hook: {}", e.command) } else { why.to_string() });
                    return out;
                }
                c => out.notes.push(format!("hook failed (exit {}): {}", c.map(|c| c.to_string()).unwrap_or_else(|| "none".into()), e.command)),
            }
        }
        out
    }
}
