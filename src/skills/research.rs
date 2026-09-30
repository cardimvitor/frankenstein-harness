//! Thin-skill research: a learned project skill that keeps being used but stays short is enriched from the
//! repository's own content (README, docs, configs, neighbouring code). Read-only, offline, one thinking-off call.
//! The proposal must cite evidence: every quote has to occur verbatim in a repo file, otherwise it is dropped, and
//! at least one verified quote is required. The result goes through the same data-only validator and version
//! history as any change; the quality police rolls it back if outcomes get worse.
use super::store::{Scope, Skill, SkillStore};
use crate::llm::client::{ChatOptions, LlmClient};
use crate::tools::fs::list_all;
use crate::types::{Message, Thinking};
use crate::util::proc::clip;
use serde_json::{json, Value};
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub const MIN_USES: i64 = 3;
pub const THIN_CHARS: usize = 420;
const COOLDOWN_MS: i64 = 7 * 24 * 3600 * 1000;

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Active learned project skills used at least `MIN_USES` times whose body is still short.
pub fn find_thin(store: &SkillStore, project_id: &str) -> Vec<Skill> {
    store
        .user_skills(Some(project_id))
        .into_iter()
        .filter(|s| s.source != "builtin" && s.state == "active" && s.scope == Scope::Project && s.body.chars().count() < THIN_CHARS)
        .filter(|s| store.conn().query_row("SELECT COUNT(*) FROM task_skills WHERE skill_id=?", [&s.id], |r| r.get::<_, i64>(0)).unwrap_or(0) >= MIN_USES)
        .collect()
}

fn researched_recently(store: &SkillStore, skill: &Skill) -> bool {
    let cutoff = now_ms() - COOLDOWN_MS;
    store.conn().query_row("SELECT COUNT(*) FROM activity WHERE kind='researched' AND skill=? AND ts>?", rusqlite::params![skill.name, cutoff], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
}

/// Repository text a proposal may draw on, each file as (relative path, excerpt).
pub fn gather(cwd: &Path, skill: &Skill) -> Vec<(String, String)> {
    let mut picked: Vec<String> = Vec::new();
    for f in ["README.md", "readme.md", "CONTRIBUTING.md", "AGENTS.md", "CLAUDE.md", ".editorconfig", "tsconfig.json", "pyproject.toml", ".eslintrc.json", "eslint.config.js", ".prettierrc", "Directory.Build.props"] {
        if cwd.join(f).is_file() {
            picked.push(f.to_string());
        }
    }
    let files = list_all(cwd, 3000);
    picked.extend(files.iter().filter(|f| f.starts_with("docs/") && f.ends_with(".md")).take(3).cloned());
    // neighbouring code: files whose path shares a word with the skill's name or keywords
    let words: Vec<String> = format!("{} {}", skill.name, skill.keywords).to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 4).map(|s| s.to_string()).collect();
    let mut near: Vec<(usize, &String)> = files.iter().filter(|f| !picked.contains(f) && !f.contains("node_modules")).map(|f| (words.iter().filter(|w| f.to_lowercase().contains(w.as_str())).count(), f)).filter(|(n, _)| *n > 0).collect();
    near.sort_by(|a, b| b.0.cmp(&a.0));
    picked.extend(near.into_iter().take(3).map(|(_, f)| f.clone()));
    let mut out = Vec::new();
    let mut total = 0usize;
    for f in picked {
        let Ok(text) = std::fs::read_to_string(cwd.join(&f)) else { continue };
        let ex: String = text.chars().take(1800).collect();
        total += ex.len();
        out.push((f, ex));
        if total > 9000 {
            break;
        }
    }
    out
}

pub fn research_schema() -> Value {
    json!({"type":"object","properties":{
        "enrich": {"type": "boolean"},
        "body": {"type": "string", "description": "the FULL improved note: 3-8 short guidance bullets"},
        "evidence": {"type": "array", "items": {"type": "object", "properties": {"file": {"type": "string"}, "quote": {"type": "string", "description": "a short verbatim quote from that file that supports the new guidance"}}, "required": ["file", "quote"]}}
    }, "required": ["enrich"]})
}

/// Quotes that really occur in the named repo file (whitespace-normalised).
pub fn verified_quotes(files: &[(String, String)], cwd: &Path, evidence: &[Value]) -> Vec<(String, String)> {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = Vec::new();
    for e in evidence {
        let (Some(f), Some(q)) = (e["file"].as_str(), e["quote"].as_str()) else { continue };
        if norm(q).chars().count() < 8 {
            continue;
        }
        let text = files.iter().find(|(p, _)| p == f).map(|(_, t)| t.clone()).or_else(|| std::fs::read_to_string(cwd.join(f)).ok());
        if text.map(|t| norm(&t).contains(&norm(q))).unwrap_or(false) {
            out.push((f.to_string(), q.to_string()));
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Enriched(String),
    Skipped(String),
}

/// Enrich one thin skill from repository content. Rate-limited to once per skill per week.
pub async fn research_skill(store: &SkillStore, llm: &LlmClient, cwd: &Path, skill: &Skill, cancel: Option<CancellationToken>) -> Outcome {
    if researched_recently(store, skill) {
        return Outcome::Skipped("researched within the last week".into());
    }
    let ctx = gather(cwd, skill);
    if ctx.is_empty() {
        return Outcome::Skipped("no repository documents to read".into());
    }
    let docs = ctx.iter().map(|(f, t)| format!("--- {f}\n{t}")).collect::<Vec<_>>().join("\n\n");
    let prompt = [
        "A private engineering guidance note about this repository is short. Improve it ONLY with conventions that the repository documents below actually state.".to_string(),
        "Return the FULL improved note (3-8 short bullet lines, guidance only: no commands, no URLs, no secrets) and, as evidence, verbatim quotes (file + exact text) that support what you added. Anything without a verbatim quote must not be added. If the documents add nothing, return {\"enrich\": false}.".to_string(),
        format!("Current note \"{}\":\n{}", skill.name, skill.body),
        format!("Repository documents:\n{}", clip(&docs, 11_000)),
    ]
    .join("\n\n");
    let r = llm.json(ChatOptions { messages: vec![Message::system("Output only JSON."), Message::user(prompt)], thinking: Thinking::Off, max_tokens: Some(900), json_schema: Some(research_schema()), cancel, ..Default::default() }).await;
    // record the attempt either way so a failing model is not retried every task
    store.log("researched", &skill.name, "read-only repository research pass", skill.project_id.as_deref());
    let Ok(r) = r else { return Outcome::Skipped("llm error".into()) };
    let Some(v) = r.value else { return Outcome::Skipped("no proposal".into()) };
    if v["enrich"].as_bool() != Some(true) {
        return Outcome::Skipped("the repository adds nothing".into());
    }
    let Some(body) = v["body"].as_str() else { return Outcome::Skipped("no body".into()) };
    let ev = v["evidence"].as_array().cloned().unwrap_or_default();
    let ok = verified_quotes(&ctx, cwd, &ev);
    if ok.is_empty() {
        return Outcome::Skipped("no verifiable evidence: proposal dropped".into());
    }
    let reason = format!("repository research, evidence: {}", ok.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>().join(", "));
    match store.improve(&skill.id, body, &reason) {
        Ok(()) => Outcome::Enriched(skill.name.clone()),
        Err(e) => Outcome::Skipped(format!("rejected: {e}")),
    }
}
