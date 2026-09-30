use crate::fingerprint::Fingerprint;
use crate::llm::client::{ChatOptions, LlmClient, LlmError};
use crate::tools::fs::list_all;
use crate::types::{Message, Thinking};
use crate::util::proc::{clip, run_simple};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Subtask {
    pub id: String,
    pub goal: String,
    pub files: Vec<String>,
    pub deps: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanStep {
    pub step: String,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Intake {
    pub trivial: bool,
    pub questions: Vec<String>,
    pub enriched: String,
    pub acceptance: Vec<String>,
    pub plan: Vec<PlanStep>,
    pub assumptions: Vec<String>,
    pub subtasks: Vec<Subtask>,
    pub skill_choice: Option<String>,
}

pub fn intake_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "trivial": {"type": "boolean", "description": "true for a small, obvious, low-risk change (one file, clear intent)"},
            "questions": {"type": "array", "items": {"type": "string"}, "description": "only questions whose answers change the implementation; max 5"},
            "enriched": {"type": "string", "description": "the task rewritten with constraints made explicit"},
            "acceptance": {"type": "array", "items": {"type": "string"}},
            "plan": {"type": "array", "items": {"type": "object", "properties": {"step": {"type": "string"}, "files": {"type": "array", "items": {"type": "string"}}}, "required": ["step", "files"]}},
            "assumptions": {"type": "array", "items": {"type": "string"}},
            "subtasks": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "goal": {"type": "string"}, "files": {"type": "array", "items": {"type": "string"}}, "deps": {"type": "array", "items": {"type": "string"}}}, "required": ["id", "goal", "files", "deps"]}, "description": "ONLY when the work splits into independent parts touching different files; otherwise empty"},
            "skill_choice": {"type": "string", "description": "when asked to choose between candidate guidance notes, the chosen name"}
        },
        "required": ["trivial", "questions", "enriched", "acceptance", "plan", "assumptions", "subtasks"]
    })
}

/// Deterministic repo inspection that feeds the planner (no model calls).
pub async fn inspect_repo(cwd: &Path, fp: &Fingerprint) -> String {
    let files = list_all(cwd, 3000);
    let mut tree = files.iter().take(80).cloned().collect::<Vec<_>>().join("\n");
    if files.len() > 80 {
        tree.push_str(&format!("\n… {} more files", files.len() - 80));
    }
    let git = run_simple("git status --short | head -15", cwd, 5000).await;
    let git = if git.code == Some(0) { git.stdout.trim().to_string() } else { String::new() };
    let readme = ["README.md", "readme.md", "README"].iter().map(|f| cwd.join(f)).find(|p| p.exists()).and_then(|p| std::fs::read_to_string(p).ok()).map(|t| t.chars().take(800).collect::<String>()).unwrap_or_default();
    let verify = if fp.verify.is_empty() { "none detected".to_string() } else { fp.verify.iter().map(|v| format!("{} ({})", v.name, v.cmd)).collect::<Vec<_>>().join("; ") };
    let mut parts = vec![format!("Stack: {}", fp.summary), format!("Verification commands: {verify}"), format!("Files:\n{tree}")];
    if !git.is_empty() {
        parts.push(format!("Uncommitted changes:\n{git}"));
    }
    if !readme.is_empty() {
        parts.push(format!("README excerpt:\n{readme}"));
    }
    parts.join("\n\n")
}

pub struct IntakeArgs<'a> {
    pub llm: &'a LlmClient,
    pub task: &'a str,
    pub repo: &'a str,
    pub auto: bool,
    pub answers: Vec<(String, String)>,
    pub ambiguous: Vec<String>,
    pub max_subtasks: usize,
    pub cancel: Option<CancellationToken>,
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str()).map(|s| s.to_string()).collect()).unwrap_or_default()
}

pub fn normalize(v: &Value) -> Intake {
    let plan = v.get("plan").and_then(|p| p.as_array()).map(|a| a.iter().filter_map(|p| Some(PlanStep { step: p.get("step")?.as_str()?.to_string(), files: strings(p.get("files")) })).collect()).unwrap_or_default();
    let subtasks = v
        .get("subtasks")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().filter_map(|s| Some(Subtask { id: s.get("id")?.as_str()?.to_string(), goal: s.get("goal")?.as_str()?.to_string(), files: strings(Some(s.get("files")?)), deps: strings(s.get("deps")) })).collect())
        .unwrap_or_default();
    let mut questions: Vec<String> = strings(v.get("questions")).into_iter().filter(|q| !q.trim().is_empty()).collect();
    questions.truncate(5);
    Intake {
        trivial: v.get("trivial").and_then(|x| x.as_bool()).unwrap_or(false),
        questions,
        enriched: v.get("enriched").and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty()).unwrap_or("").to_string(),
        acceptance: strings(v.get("acceptance")),
        plan,
        assumptions: strings(v.get("assumptions")),
        subtasks,
        skill_choice: v.get("skill_choice").and_then(|x| x.as_str()).map(|s| s.to_string()),
    }
}

/// One structured planner call (thinking on). Guided mode may return questions; auto mode never asks, it records assumptions.
pub async fn intake(a: IntakeArgs<'_>) -> Result<Intake, LlmError> {
    let rules = if a.auto {
        "Do NOT ask questions. Resolve ambiguity yourself and list each assumption in \"assumptions\"."
    } else {
        "Ask at most 5 questions, only when the answer changes the implementation and cannot be found in the repository. If you can infer it, do not ask."
    };
    let mut prompt: Vec<String> = vec![
        "You are the planning stage of a coding agent. Inspect the summary below and produce a structured plan.".into(),
        rules.into(),
        "Write acceptance criteria that can be checked (behavior, tests, files that must not change).".into(),
        "Plan steps must name the files they touch. Set trivial=true only for a small obvious change.".into(),
        format!("Only split into \"subtasks\" when the parts are independent and touch DISJOINT files; otherwise leave subtasks empty.{}", if a.max_subtasks > 0 { format!(" At most {}.", a.max_subtasks) } else { String::new() }),
    ];
    if !a.ambiguous.is_empty() {
        prompt.push(format!("If relevant, pick one guidance note and put its name in skill_choice: {}", a.ambiguous.join(" | ")));
    }
    if !a.answers.is_empty() {
        prompt.push(format!("Answers already given:\n{}\nDo not ask further questions.", a.answers.iter().map(|(q, ans)| format!("Q: {q}\nA: {ans}")).collect::<Vec<_>>().join("\n")));
    }
    prompt.push(format!("Task:\n{}", a.task));
    prompt.push(format!("Repository:\n{}", clip(a.repo, 7000)));
    let r = a
        .llm
        .json(ChatOptions { messages: vec![Message::system("You output only JSON matching the schema."), Message::user(prompt.join("\n\n"))], thinking: Thinking::High, max_tokens: Some(3500), json_schema: Some(intake_schema()), cancel: a.cancel, ..Default::default() })
        .await?;
    let mut out = normalize(&r.value.unwrap_or(Value::Null));
    if out.enriched.is_empty() {
        out.enriched = a.task.to_string();
    }
    if a.auto || !a.answers.is_empty() {
        out.questions.clear();
    }
    Ok(out)
}

pub fn render_plan(i: &Intake) -> String {
    let lines: Vec<String> = i.plan.iter().enumerate().map(|(n, p)| format!("{}. {}{}", n + 1, p.step, if p.files.is_empty() { String::new() } else { format!("  [{}]", p.files.join(", ")) })).collect();
    let acc = if i.acceptance.is_empty() { String::new() } else { format!("\nDone when:\n{}", i.acceptance.iter().map(|x| format!("- {x}")).collect::<Vec<_>>().join("\n")) };
    let ass = if i.assumptions.is_empty() { String::new() } else { format!("\nAssumptions:\n{}", i.assumptions.iter().map(|x| format!("- {x}")).collect::<Vec<_>>().join("\n")) };
    format!("{}{}{}", if lines.is_empty() { "(no steps)".to_string() } else { lines.join("\n") }, acc, ass)
}
