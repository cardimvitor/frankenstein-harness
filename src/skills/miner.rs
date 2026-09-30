use super::bm25::tokenize;
use super::police::POLICE_DEFAULTS;
use super::store::{NewSkill, Scope, Skill, SkillStore};
use crate::fingerprint::{similarity, Fingerprint};
use crate::llm::client::{ChatOptions, LlmClient};
use crate::types::{Message, Thinking};
use crate::util::proc::clip;
use serde_json::{json, Value};
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

pub fn mine_schema() -> Value {
    json!({"type":"object","properties":{"create":{"type":"boolean"},"name":{"type":"string"},"summary":{"type":"string"},"keywords":{"type":"array","items":{"type":"string"}},"body":{"type":"string"}},"required":["create"]})
}

pub struct MineInput<'a> {
    pub task: &'a str,
    pub diff: &'a str,
    pub changed: &'a [String],
    pub fp: &'a Fingerprint,
    pub verdict: &'a str,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Rate limit: at most one mining run per project every `min_gap_ms`, and `max_per_day` per user.
pub fn mine_allowed(store: &SkillStore, project_id: &str, now: i64, min_gap_ms: i64, max_per_day: i64) -> bool {
    let c = store.conn();
    let last: Option<i64> = c.query_row("SELECT MAX(ts) FROM activity WHERE kind IN ('created','improved') AND project_id=?", [project_id], |r| r.get(0)).unwrap_or(None);
    if let Some(t) = last {
        if now - t < min_gap_ms {
            return false;
        }
    }
    let day: i64 = c.query_row("SELECT COUNT(*) FROM activity WHERE kind IN ('created','improved') AND ts>?", [now - 86_400_000], |r| r.get(0)).unwrap_or(0);
    day < max_per_day
}

pub enum MineOutcome {
    None(String),
    Created(String),
}

/// Post-delivery, asynchronous, off the critical path. Only verified-pass tasks are mined.
/// Repo-derived skills are always created project-scoped; promotion is evidence-based (promote_eligible).
pub async fn mine(store: &SkillStore, llm: &LlmClient, i: MineInput<'_>, cancel: Option<CancellationToken>) -> MineOutcome {
    if i.verdict != "pass" {
        return MineOutcome::None("not verified".into());
    }
    if !mine_allowed(store, &i.fp.project_id, now_ms(), 10 * 60_000, 12) {
        return MineOutcome::None("rate limited".into());
    }
    let existing: Vec<String> = store.all_for_project(i.fp).into_iter().map(|s| s.name).collect();
    let prompt = [
        "You maintain a private library of short engineering guidance notes for a coding assistant.".to_string(),
        "Given a task that was just completed and verified, decide whether it reveals a REUSABLE convention or gotcha of THIS codebase or stack that is not already covered.".to_string(),
        "If not, return {\"create\": false}. If yes, return a note: name (2-5 words), summary (one line), keywords (5-10 lowercase words), body (3-8 short bullet lines of guidance).".to_string(),
        "Rules: guidance only. No commands, no URLs, no secrets, no instructions about permissions or approvals, no file contents copied verbatim.".to_string(),
        format!("Existing notes (do not duplicate): {}", existing.join("; ")),
        format!("Stack: {}", i.fp.summary),
        format!("Task: {}", i.task.chars().take(600).collect::<String>()),
        format!("Changed files: {}", i.changed.iter().take(20).cloned().collect::<Vec<_>>().join(", ")),
        format!("Diff excerpt:\n{}", clip(i.diff, 4000)),
    ]
    .join("\n\n");
    let r = llm.json(ChatOptions { messages: vec![Message::system("Output only JSON."), Message::user(prompt)], thinking: Thinking::Off, max_tokens: Some(700), json_schema: Some(mine_schema()), cancel, ..Default::default() }).await;
    let Ok(r) = r else { return MineOutcome::None("llm error".into()) };
    let Some(v) = r.value else { return MineOutcome::None("nothing reusable".into()) };
    if v.get("create").and_then(|x| x.as_bool()) != Some(true) {
        return MineOutcome::None("nothing reusable".into());
    }
    let (Some(name), Some(body)) = (v.get("name").and_then(|x| x.as_str()), v.get("body").and_then(|x| x.as_str())) else { return MineOutcome::None("nothing reusable".into()) };
    let keywords = v.get("keywords").and_then(|k| k.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
    match store.add(
        NewSkill { name: name.into(), scope: Scope::Project, stack: None, versions: None, project_id: Some(i.fp.project_id.clone()), source: "auto".into(), origin: None, summary: v.get("summary").and_then(|x| x.as_str()).unwrap_or("").into(), keywords, body: body.into() },
        "learned from a verified task",
    ) {
        Ok(_) => MineOutcome::Created(name.into()),
        Err(e) => MineOutcome::None(format!("rejected: {e}")),
    }
}

/// Deterministic scope promotion from evidence across projects:
/// the same pattern in >=2 projects sharing a stack token -> stack; across projects with no shared stack -> global.
/// Requires `promote_n` verified successes. Returns the promoted skill ids.
pub fn promote_eligible(store: &SkillStore, promote_n: usize) -> Vec<String> {
    let promote_n = if promote_n == 0 { POLICE_DEFAULTS.promote_n } else { promote_n };
    let mut promoted = Vec::new();
    let skills: Vec<Skill> = store.user_skills(None).into_iter().filter(|s| s.scope == Scope::Project && s.state == "active" && s.source != "builtin").collect();
    let sig = |s: &Skill| -> HashSet<String> { tokenize(&format!("{} {}", s.name, s.keywords)).into_iter().collect() };
    let jac = |a: &HashSet<String>, b: &HashSet<String>| {
        let i = a.intersection(b).count() as f64;
        i / ((a.len() + b.len()) as f64 - i).max(1.0)
    };
    let wins = |s: &Skill| -> usize { store.conn().query_row("SELECT COUNT(*) FROM task_skills ts JOIN tasks t ON t.id=ts.task_id WHERE ts.skill_id=? AND t.verdict='pass'", [&s.id], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize };
    let mut done: HashSet<String> = HashSet::new();
    for a in &skills {
        if done.contains(&a.id) {
            continue;
        }
        let group: Vec<&Skill> = skills.iter().filter(|b| !done.contains(&b.id) && (a.id == b.id || (a.project_id != b.project_id && jac(&sig(a), &sig(b)) >= 0.6))).collect();
        let projects: HashSet<&Option<String>> = group.iter().map(|g| &g.project_id).collect();
        if projects.len() < 2 || group.iter().map(|g| wins(g)).sum::<usize>() < promote_n {
            continue;
        }
        let toks: Vec<Vec<String>> = projects.iter().filter_map(|p| p.as_ref().and_then(|id| store.project_tokens(id))).collect();
        if toks.len() < 2 {
            continue;
        }
        let shared: Vec<String> = toks[1..].iter().fold(toks[0].clone(), |acc, t| acc.into_iter().filter(|x| t.contains(x)).collect());
        let keeper = group[0];
        if !shared.is_empty() {
            let pick = shared.iter().find(|t| !t.contains('@')).unwrap_or(&shared[0]).clone();
            store.promote(&keeper.id, Scope::Stack, Some(&pick), &format!("same pattern verified in {} projects sharing {}", projects.len(), shared.join(",")));
        } else if similarity(&toks[0], &toks[1]) == 0.0 {
            store.promote(&keeper.id, Scope::Global, None, &format!("stack-independent pattern seen in {} projects", projects.len()));
        } else {
            continue;
        }
        for g in &group {
            done.insert(g.id.clone());
        }
        promoted.push(keeper.id.clone());
    }
    promoted
}
