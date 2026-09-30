use super::bm25::{tokenize, Bm25, Doc};
use super::store::{Scope, Skill, SkillStore};
use super::usercfg::{UserConfig, UserSkill};
use crate::fingerprint::Fingerprint;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct GateResult {
    pub decision: &'static str,
    pub selected: Vec<Skill>,
    pub user_skills: Vec<UserSkill>,
    /// Set when the deterministic match is unclear; the planner resolves it inside its structured output (no extra turn).
    pub ambiguous: Vec<String>,
    pub ms: f64,
    pub llm_calls: u32,
}

fn boost(s: Scope) -> f64 {
    match s {
        Scope::Project => 1.3,
        Scope::Stack => 1.15,
        Scope::Global => 1.0,
    }
}

const MAX_SELECTED: usize = 3;
const MIN_SCORE: f64 = 1.5;
const MIN_TASK_WORDS: usize = 4;

/// Deterministic skill gate: repo fingerprint decides which skills apply, BM25 over the task text ranks them.
/// Never calls the model. Stack packs that match the repo are always eligible so version guidance is not lost
/// when the task text does not mention the framework.
pub fn gate(store: &SkillStore, fp: &Fingerprint, task: &str, user: &UserConfig) -> GateResult {
    let t0 = Instant::now();
    let pool = store.all_for_project(fp);
    let bm = Bm25::new(pool.iter().map(|s| Doc { id: s.id.clone(), text: format!("{} {} {}", s.name, s.summary, s.body), boost: format!("{} {} {}", s.name, s.keywords, s.stack.clone().unwrap_or_default()) }).collect());
    // The query is the task text only: the fingerprint decides which skills APPLY and gives stack packs their bonus below.
    let q = tokenize(task);
    let scores = bm.score(&q);
    let mut ranked: Vec<(Skill, f64)> = pool
        .into_iter()
        .map(|s| {
            let mut sc = scores.get(&s.id).copied().unwrap_or(0.0) * boost(s.scope);
            if s.scope == Scope::Stack {
                sc += 1.2;
            }
            if s.scope == Scope::Project {
                sc += 0.6;
            }
            (s, sc)
        })
        .filter(|(_, sc)| *sc >= MIN_SCORE)
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut selected: Vec<Skill> = Vec::new();
    let mut globals = 0;
    for (s, sc) in &ranked {
        if selected.len() >= MAX_SELECTED {
            break;
        }
        if s.scope == Scope::Global && globals >= 2 && *sc < ranked[0].1 * 0.6 {
            continue;
        }
        if s.scope == Scope::Global {
            globals += 1;
        }
        selected.push(s.clone());
    }
    let gr: Vec<&(Skill, f64)> = ranked.iter().filter(|(s, _)| s.scope == Scope::Global).collect();
    let ambiguous = if gr.len() >= 2 && gr[0].1 - gr[1].1 < gr[0].1 * 0.1 && gr[0].1 < 3.0 { vec![gr[0].0.name.clone(), gr[1].0.name.clone()] } else { vec![] };

    // user SKILL.md skills matched by description/name
    let ubm = Bm25::new(user.skills.iter().map(|u| Doc { id: u.path.clone(), text: format!("{} {} {}", u.name, u.description, u.body.chars().take(400).collect::<String>()), boost: format!("{} {}", u.name, u.description) }).collect());
    let us = ubm.score(&q);
    let user_skills: Vec<UserSkill> = user.skills.iter().filter(|u| us.get(&u.path).copied().unwrap_or(0.0) >= 1.2).take(3).cloned().collect();

    let words = tokenize(task).len();
    let decision = if selected.iter().any(|s| s.scope != Scope::Global) || !user_skills.is_empty() || !selected.is_empty() { "use" } else if words >= MIN_TASK_WORDS { "create" } else { "use" };
    GateResult { decision, selected, user_skills, ambiguous, ms: t0.elapsed().as_secs_f64() * 1000.0, llm_calls: 0 }
}

pub fn render_skills(g: &GateResult) -> String {
    g.selected.iter().map(|s| format!("## {}\n{}", s.name, s.body)).collect::<Vec<_>>().join("\n\n")
}

/// User rules and matched user skills; placed last and marked higher priority.
pub fn render_user(user: &UserConfig, g: &GateResult) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !user.rules.is_empty() {
        parts.push(user.rules.clone());
    }
    for u in &g.user_skills {
        parts.push(format!("# Skill: {}\n{}", u.name, u.body));
    }
    parts.join("\n\n")
}
