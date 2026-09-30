use crate::llm::client::{ChatOptions, LlmClient};
use crate::types::{Message, Thinking};
use crate::util::proc::clip;
use serde_json::{json, Value};
use std::path::Path;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub severity: String,
    pub claim: String,
    pub quote: String,
}

pub fn review_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["pass", "fail"]},
            "findings": {"type": "array", "items": {"type": "object", "properties": {
                "file": {"type": "string"}, "line": {"type": "integer"}, "severity": {"type": "string", "enum": ["blocker", "major", "minor"]},
                "claim": {"type": "string"}, "quote": {"type": "string", "description": "verbatim text of the cited line"}
            }, "required": ["file", "line", "severity", "claim", "quote"]}}
        },
        "required": ["verdict", "findings"]
    })
}

pub const CHECKLISTS: [&str; 5] = [
    "Correctness: does the change do what the task and acceptance criteria require? Look for logic errors, wrong conditions, off-by-one, unhandled null/undefined.",
    "Edge cases and error handling: empty input, failures, concurrency, resource cleanup, missing tests for the new behavior.",
    "Security: injection, path traversal, unsafe deserialization, secrets, authz checks, unsafe shell/SQL construction.",
    "Regressions and scope: broken callers, changed public API, unrelated edits, inconsistent style with surrounding code.",
    "Final pass: completeness against every acceptance criterion, leftover debug code, performance traps (N+1, quadratic loops).",
];

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Drop findings that cannot be checked: unknown file, out-of-range line, or a quote that is not near that line.
pub fn validate_findings(cwd: &Path, raw: &Value, changed: &[String]) -> (Vec<Finding>, usize) {
    let mut valid = Vec::new();
    let mut dropped = 0;
    for f in raw.as_array().cloned().unwrap_or_default() {
        let (Some(file), Some(line), Some(quote), Some(claim), Some(sev)) = (
            f.get("file").and_then(|v| v.as_str()),
            f.get("line").and_then(|v| v.as_i64()),
            f.get("quote").and_then(|v| v.as_str()),
            f.get("claim").and_then(|v| v.as_str()),
            f.get("severity").and_then(|v| v.as_str()),
        ) else {
            dropped += 1;
            continue;
        };
        if quote.trim().chars().count() < 3 || !["blocker", "major", "minor"].contains(&sev) {
            dropped += 1;
            continue;
        }
        let file = file.trim_start_matches("./").to_string();
        let p = cwd.join(&file);
        if !changed.contains(&file) || !p.exists() {
            dropped += 1;
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let lines: Vec<&str> = text.split('\n').collect();
        if line < 1 || line as usize > lines.len() {
            dropped += 1;
            continue;
        }
        let ln = line as usize;
        let lo = ln.saturating_sub(2);
        let hi = (ln + 1).min(lines.len());
        if !norm(&lines[lo..hi].join(" ")).contains(&norm(quote)) {
            dropped += 1;
            continue;
        }
        valid.push(Finding { file, line: ln, severity: sev.to_string(), claim: claim.chars().take(400).collect(), quote: quote.chars().take(200).collect() });
    }
    (valid, dropped)
}

pub struct ReviewOut {
    pub valid: Vec<Finding>,
    pub dropped: usize,
    pub malformed: bool,
}

pub async fn review(llm: &LlmClient, cwd: &Path, diff: &str, changed: &[String], acceptance: &[String], checklist: &str, cancel: Option<CancellationToken>) -> Result<ReviewOut, crate::llm::client::LlmError> {
    let acc = if acceptance.is_empty() { "- (none given)".to_string() } else { acceptance.iter().map(|a| format!("- {a}")).collect::<Vec<_>>().join("\n") };
    let prompt = [
        "You are a strict code reviewer. Review ONLY the diff below against the task criteria and the checklist.".to_string(),
        "Report only concrete defects you can point to. For each, give file, line (line number in the NEW file), severity, a one-sentence claim, and quote = the verbatim text of that line.".to_string(),
        "If there are no defects, verdict \"pass\" with an empty findings list. Do not report style preferences as blockers.".to_string(),
        format!("Checklist for this round: {checklist}"),
        format!("Acceptance criteria:\n{acc}"),
        format!("Diff:\n{}", clip(diff, 24000)),
    ]
    .join("\n\n");
    let r = llm
        .json(ChatOptions { messages: vec![Message::system("You output only JSON matching the schema."), Message::user(prompt)], thinking: Thinking::High, max_tokens: Some(3000), json_schema: Some(review_schema()), cancel, ..Default::default() })
        .await?;
    let Some(v) = r.value else { return Ok(ReviewOut { valid: vec![], dropped: 0, malformed: true }) };
    let (valid, dropped) = validate_findings(cwd, v.get("findings").unwrap_or(&Value::Null), changed);
    Ok(ReviewOut { valid, dropped, malformed: false })
}
