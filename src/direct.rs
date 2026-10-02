//! `fh direct "<task>"`: the model alone, no harness.
//!
//! One request carries a snapshot of the repository and the task; one reply carries the edits; they are applied as
//! written. No tools, no agent loop, no verification, no skills, no second attempt. It exists to be the baseline that
//! every harness (fh included) is measured against: did the harness make the model better or worse?
//!
//! Reply format (the same SEARCH/REPLACE blocks coding models already know):
//!
//! ```text
//! path/to/file.py
//! <<<<<<< SEARCH
//! exact existing lines
//! =======
//! replacement lines
//! >>>>>>> REPLACE
//! ```
//! An empty SEARCH creates a new file. The SEARCH text must match exactly once.

use crate::config::{Config, Env};
use crate::llm::client::{ChatOptions, LlmClient};
use crate::tools::fs::{list_all, locate};
use crate::types::{Message, Thinking};
use crate::util::paths::in_workspace;
use serde_json::{json, Value};
use std::path::Path;

pub const SYSTEM: &str = "You are an expert software engineer. You are given a snapshot of a repository and a task. You cannot run code or commands. Reply ONLY with the edits that solve the task, as SEARCH/REPLACE blocks, one block per change:\n\npath/to/file\n<<<<<<< SEARCH\nexact existing lines to replace (copy them exactly)\n=======\nreplacement lines\n>>>>>>> REPLACE\n\nRules: the path is relative to the repository root and goes on the line before <<<<<<< SEARCH; the SEARCH text must match the file exactly once, so include enough surrounding lines; to create a new file leave SEARCH empty; keep public APIs unless the task says otherwise; do not add explanations outside the blocks.";

#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// Extracts SEARCH/REPLACE blocks from a model reply (tolerates code fences and `File:` prefixes around the path).
pub fn parse_edits(reply: &str) -> Vec<Edit> {
    let lines: Vec<&str> = reply.lines().collect();
    let mut edits = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_end() == "<<<<<<< SEARCH" {
            // the path is the closest earlier line that is not blank or a code fence
            let mut path = String::new();
            let mut k = i;
            while k > 0 {
                k -= 1;
                let l = lines[k].trim();
                if l.is_empty() || l.starts_with("```") {
                    continue;
                }
                path = l.trim_start_matches("File:").trim_start_matches("file:").trim().trim_end_matches(':').trim_matches(|c| c == '`' || c == '*' || c == '"' || c == '\'').trim().to_string();
                break;
            }
            let (mut search, mut replace) = (Vec::new(), Vec::new());
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim_end() != "=======" {
                search.push(lines[j]);
                j += 1;
            }
            j += 1;
            while j < lines.len() && lines[j].trim_end() != ">>>>>>> REPLACE" {
                replace.push(lines[j]);
                j += 1;
            }
            if j < lines.len() && !path.is_empty() {
                edits.push(Edit { path, search: search.join("\n"), replace: replace.join("\n") });
            }
            i = j;
        }
        i += 1;
    }
    edits
}

/// Applies the edits in order. Returns the files changed and the edits that could not be applied (with the reason).
pub fn apply_edits(cwd: &Path, edits: &[Edit]) -> (Vec<String>, Vec<(String, String)>) {
    let (mut changed, mut failed) = (Vec::<String>::new(), Vec::new());
    for e in edits {
        let abs = match in_workspace(cwd, &e.path) {
            Ok(p) => p,
            Err(err) => {
                failed.push((e.path.clone(), err.to_string()));
                continue;
            }
        };
        let existing = std::fs::read_to_string(&abs).ok();
        let result: Result<String, String> = match (&existing, e.search.trim().is_empty()) {
            (None, true) => Ok(e.replace.clone()),
            (Some(_), true) => Err("file exists: an empty SEARCH only creates new files".into()),
            (None, false) => Err("file does not exist".into()),
            (Some(text), false) => match locate(text, &e.search) {
                None => Err("SEARCH text not found".into()),
                Some((_, _, n, _)) if n > 1 => Err(format!("SEARCH text matches {n} times")),
                Some((s, en, _, _)) => Ok(format!("{}{}{}", &text[..s], e.replace, &text[en..])),
            },
        };
        match result {
            Ok(new_text) => {
                if let Some(parent) = abs.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let nt = if existing.as_deref().map(|t| t.ends_with('\n')).unwrap_or(true) && !new_text.ends_with('\n') { format!("{new_text}\n") } else { new_text };
                match std::fs::write(&abs, nt) {
                    Ok(_) => {
                        if !changed.contains(&e.path) {
                            changed.push(e.path.clone());
                        }
                    }
                    Err(err) => failed.push((e.path.clone(), err.to_string())),
                }
            }
            Err(why) => failed.push((e.path.clone(), why)),
        }
    }
    (changed, failed)
}

const MAX_FILE_CHARS: usize = 60_000;

/// Repository snapshot for the prompt: the file tree, then file contents until the budget is used. Order of contents:
/// root documentation (README, CHALLENGE...), files named in the task, then the remaining text files smallest first.
pub fn build_context(cwd: &Path, task: &str, budget_chars: usize) -> (String, usize, usize) {
    let all = list_all(cwd, 5000);
    let mut out = String::from("Repository files:\n");
    for f in all.iter().take(1500) {
        out.push_str(f);
        out.push('\n');
    }
    if all.len() > 1500 {
        out.push_str(&format!("... and {} more\n", all.len() - 1500));
    }
    let task_l = task.to_lowercase();
    let is_doc = |f: &str| !f.contains('/') && { let u = f.to_uppercase(); u.starts_with("README") || u.starts_with("CHALLENGE") || u.starts_with("CONTRIBUTING") || u.starts_with("AGENTS") };
    let mentioned = |f: &str| {
        let base = f.rsplit('/').next().unwrap_or(f).to_lowercase();
        task_l.contains(&f.to_lowercase()) || (base.len() >= 5 && task_l.contains(&base))
    };
    let size = |f: &String| std::fs::metadata(cwd.join(f)).map(|m| m.len()).unwrap_or(u64::MAX);
    let mut order: Vec<&String> = all.iter().filter(|f| is_doc(f)).collect();
    order.extend(all.iter().filter(|f| !is_doc(f) && mentioned(f)));
    let mut rest: Vec<&String> = all.iter().filter(|f| !is_doc(f) && !mentioned(f)).collect();
    rest.sort_by_key(|f| size(f));
    order.extend(rest);
    let (mut used, mut included) = (out.len(), 0usize);
    for f in order {
        let Ok(text) = std::fs::read_to_string(cwd.join(f)) else { continue };
        if text.is_empty() || text.len() > MAX_FILE_CHARS || text.contains('\0') {
            continue;
        }
        let block = format!("\n=== {f} ===\n{text}\n");
        if used + block.len() > budget_chars {
            continue;
        }
        used += block.len();
        included += 1;
        out.push_str(&block);
    }
    (out, included, all.len())
}

/// Runs the baseline in `cwd`. The returned JSON has the same shape as `fh run --json` where it matters
/// (verdict, final, changed, llm), so the Harbor/Pier adapters read it unchanged.
pub async fn run_direct(cfg: &Config, env: &Env, cwd: &Path, task: &str, commit: bool) -> Value {
    let llm = LlmClient::new(cfg.clone(), env.clone());
    let budget = (cfg.context_window as f64 * 3.0 * 0.6) as usize; // ~60% of the window, 3 chars per token on code
    let (snapshot, included, total) = build_context(cwd, task, budget);
    let user = format!("{snapshot}\n\nTask:\n{task}");
    let t0 = std::time::Instant::now();
    let max_tokens = (cfg.context_window as u32 / 4).clamp(4096, 32768);
    let res = llm.chat(ChatOptions { messages: vec![Message::system(SYSTEM), Message::user(user)], thinking: Thinking::High, max_tokens: Some(max_tokens), ..Default::default() }).await;
    let stats = llm.stats();
    let llm_json = json!({"requests": stats.requests, "promptTokens": stats.prompt_tokens, "completionTokens": stats.completion_tokens, "cachedTokens": stats.cached_tokens, "toolCalls": 0, "repaired": 0, "malformed": 0, "thinkLeaks": stats.think_leaks, "retries": stats.retries});
    let reply = match res {
        Ok(r) => r.content,
        Err(e) => return json!({"verdict": "error", "reason": e.to_string(), "final": "", "changed": [], "rounds": 0, "workers": [], "llm": llm_json, "direct": {"contextFiles": included, "repoFiles": total}}),
    };
    let edits = parse_edits(&reply);
    let (changed, failed) = apply_edits(cwd, &edits);
    let verdict = if !changed.is_empty() && failed.is_empty() { "applied" } else { "failed" };
    let reason = if edits.is_empty() { "the reply contained no SEARCH/REPLACE blocks".to_string() } else if failed.is_empty() { format!("applied {} edit(s)", edits.len()) } else { format!("{} of {} edit(s) could not be applied", failed.len(), edits.len()) };
    if commit && !changed.is_empty() {
        crate::util::proc::run_simple("git add -A && git -c user.name=fh -c user.email=fh@local commit -qm 'direct: model-only edits'", cwd, 30_000).await;
    }
    json!({
        "verdict": verdict, "reason": reason, "final": reply.chars().take(3000).collect::<String>(), "changed": changed, "rounds": 0, "workers": [],
        "llm": llm_json,
        "timings": {"totalMs": t0.elapsed().as_millis() as u64},
        "direct": {"edits": edits.len(), "failed": failed.iter().map(|(p, w)| json!({"path": p, "why": w})).collect::<Vec<_>>(), "contextFiles": included, "repoFiles": total},
    })
}
