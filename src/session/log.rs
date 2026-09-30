//! Append-only session transcript per task run, under `<data dir>/sessions/<project id>/`.
//! `<id>.jsonl` holds the events (start, plan, checkpoint, result); `<id>.history.json` is the
//! agent's message history, rewritten after every step so an interrupted run can be resumed.
use crate::config::{data_dir, Env};
use crate::funnel::intake::Intake;
use crate::types::Message;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone)]
pub struct SessionLog {
    pub id: String,
    dir: PathBuf,
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub fn sessions_dir(env: &Env, project_id: &str) -> PathBuf {
    data_dir(env).join("sessions").join(project_id)
}

impl SessionLog {
    pub fn create(env: &Env, project_id: &str) -> SessionLog {
        let dir = sessions_dir(env, project_id);
        let _ = std::fs::create_dir_all(&dir);
        SessionLog { id: format!("{:013}", now_ms()), dir }
    }

    /// Continue writing to an existing session (resume).
    pub fn open(env: &Env, project_id: &str, id: &str) -> SessionLog {
        SessionLog { id: id.to_string(), dir: sessions_dir(env, project_id) }
    }

    pub fn append(&self, v: Value) {
        let mut v = v;
        if let Some(o) = v.as_object_mut() {
            o.insert("ts".into(), json!(now_ms() as u64));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(format!("{}.jsonl", self.id))) {
            let _ = writeln!(f, "{v}");
        }
    }

    pub fn save_history(&self, msgs: &[Message]) {
        let clipped: Vec<Message> = msgs
            .iter()
            .map(|m| {
                let mut m = m.clone();
                if let Some(c) = &m.content {
                    if c.chars().count() > 20_000 {
                        m.content = Some(c.chars().take(20_000).collect::<String>() + "\n[clipped]");
                    }
                }
                m
            })
            .collect();
        let tmp = self.dir.join(format!("{}.history.tmp", self.id));
        if std::fs::write(&tmp, serde_json::to_string(&clipped).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, self.dir.join(format!("{}.history.json", self.id)));
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub task: String,
    pub auto: bool,
    pub plan: Option<Intake>,
    pub checkpoint: Option<String>,
    /// None while the run has not finished (interrupted)
    pub verdict: Option<String>,
    pub reason: String,
    pub history: Option<Vec<Message>>,
    pub started: u64,
    /// the delivered answer and the files changed (empty for sessions recorded before these were kept)
    pub final_text: String,
    pub changed: Vec<String>,
}

pub fn parse(dir: &std::path::Path, id: &str) -> Option<SessionInfo> {
    let text = std::fs::read_to_string(dir.join(format!("{id}.jsonl"))).ok()?;
    let mut info = SessionInfo { id: id.to_string(), task: String::new(), auto: false, plan: None, checkpoint: None, verdict: None, reason: String::new(), history: None, started: 0, final_text: String::new(), changed: vec![] };
    for l in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
        match v["t"].as_str() {
            Some("start") => {
                info.task = v["task"].as_str().unwrap_or("").to_string();
                info.auto = v["auto"].as_bool().unwrap_or(false);
                info.started = v["ts"].as_u64().unwrap_or(0);
            }
            Some("plan") => info.plan = serde_json::from_value(v["plan"].clone()).ok(),
            Some("checkpoint") => info.checkpoint = v["id"].as_str().map(|s| s.to_string()),
            Some("result") => {
                info.verdict = v["verdict"].as_str().map(|s| s.to_string());
                info.reason = v["reason"].as_str().unwrap_or("").to_string();
                info.final_text = v["final"].as_str().unwrap_or("").to_string();
                info.changed = v["changed"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            }
            _ => {}
        }
    }
    info.history = std::fs::read_to_string(dir.join(format!("{id}.history.json"))).ok().and_then(|s| serde_json::from_str(&s).ok());
    Some(info)
}

/// Newest first.
pub fn list(env: &Env, project_id: &str) -> Vec<SessionInfo> {
    let dir = sessions_dir(env, project_id);
    let mut ids: Vec<String> = std::fs::read_dir(&dir).map(|rd| rd.flatten().filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".jsonl").map(|s| s.to_string())).collect()).unwrap_or_default();
    ids.sort();
    ids.reverse();
    ids.iter().filter_map(|i| parse(&dir, i)).collect()
}

pub fn load(env: &Env, project_id: &str, id: Option<&str>) -> Option<SessionInfo> {
    match id {
        Some(i) => parse(&sessions_dir(env, project_id), i),
        None => list(env, project_id).into_iter().next(),
    }
}
