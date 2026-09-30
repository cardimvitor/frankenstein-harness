//! Search across past task sessions (this repository or all of them): what was asked, the plan, what was delivered,
//! the verdict and the files touched. BM25 ranking over the session logs already kept on disk; nothing is indexed
//! elsewhere and nothing leaves the machine.
use super::log::{parse, SessionInfo};
use crate::config::{data_dir, redact, Env};
use crate::skills::bm25::{tokenize, Bm25, Doc};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Hit {
    pub project: String,
    pub id: String,
    pub started: u64,
    pub verdict: Option<String>,
    pub task: String,
    pub snippet: String,
    pub score: f64,
}

fn session_text(s: &SessionInfo) -> String {
    let mut t = vec![s.task.clone()];
    if let Some(p) = &s.plan {
        t.push(p.enriched.clone());
        t.extend(p.acceptance.iter().cloned());
        t.extend(p.plan.iter().map(|x| format!("{} {}", x.step, x.files.join(" "))));
    }
    t.push(s.final_text.clone());
    t.push(s.reason.clone());
    t.push(s.changed.join(" "));
    t.join("\n")
}

fn snippet(text: &str, query: &[String]) -> String {
    let lower = text.to_lowercase();
    let at = query.iter().filter_map(|q| lower.find(q.as_str())).min().unwrap_or(0);
    let start = lower[..at].char_indices().rev().nth(70).map(|(i, _)| i).unwrap_or(0);
    let s: String = text[start..].chars().take(200).collect();
    format!("{}{}", if start > 0 { "…" } else { "" }, s.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Ranked sessions matching `query`. `project_id`: only that repository, None = every repository.
pub fn search(env: &Env, project_id: Option<&str>, query: &str, limit: usize) -> Vec<Hit> {
    let q = tokenize(query);
    if q.is_empty() {
        return vec![];
    }
    let root: PathBuf = data_dir(env).join("sessions");
    let projects: Vec<String> = match project_id {
        Some(p) => vec![p.to_string()],
        None => std::fs::read_dir(&root).map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default(),
    };
    let mut sessions: Vec<(String, SessionInfo, String)> = Vec::new();
    for p in projects {
        let dir = root.join(&p);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Some(id) = e.file_name().to_string_lossy().strip_suffix(".jsonl").map(|s| s.to_string()) else { continue };
            if let Some(info) = parse(&dir, &id) {
                let text = session_text(&info);
                sessions.push((p.clone(), info, text));
            }
        }
    }
    let docs: Vec<Doc> = sessions.iter().map(|(p, i, t)| Doc { id: format!("{p}/{}", i.id), text: t.clone(), boost: i.task.clone() }).collect();
    let scores = Bm25::new(docs).score(&q);
    let mut hits: Vec<Hit> = sessions
        .into_iter()
        .filter_map(|(p, i, t)| {
            let sc = scores.get(&format!("{p}/{}", i.id)).copied().unwrap_or(0.0);
            (sc > 0.0).then(|| Hit { snippet: redact(&snippet(&t, &q), env), task: redact(&i.task.chars().take(160).collect::<String>(), env), project: p, id: i.id, started: i.started, verdict: i.verdict, score: sc })
        })
        .collect();
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then(b.started.cmp(&a.started)));
    hits.truncate(limit);
    hits
}
