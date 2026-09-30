use crate::llm::client::{ChatOptions, LlmClient};
use crate::types::{Message, Thinking};
use tokio_util::sync::CancellationToken;

const SUMMARY_TAG: &str = "[Summary of earlier steps]";

pub fn est_tokens(m: &[Message]) -> usize {
    let chars: usize = m.iter().map(|x| x.content.as_ref().map(|c| c.chars().count()).unwrap_or(0) + x.tool_calls.as_ref().map(|t| serde_json::to_string(t).map(|s| s.len()).unwrap_or(0)).unwrap_or(0) + 8).sum();
    (chars as f64 / 3.4).ceil() as usize
}

/// Deterministic compaction (no extra LLM call): prune old tool outputs first, then drop the oldest
/// turns after the first user message. Keeps assistant/tool pairs intact. Returns (messages, pruned count).
pub fn compact(messages: &[Message], budget: usize, keep_recent_tools: usize) -> (Vec<Message>, usize) {
    let mut out: Vec<Message> = messages.to_vec();
    let mut pruned = 0;
    if est_tokens(&out) <= budget {
        return (out, 0);
    }
    let tool_idx: Vec<usize> = out.iter().enumerate().filter(|(_, m)| m.role == "tool").map(|(i, _)| i).collect();
    let n_old = tool_idx.len().saturating_sub(keep_recent_tools);
    for &i in tool_idx.iter().take(n_old) {
        let len = out[i].content.as_ref().map(|c| c.chars().count()).unwrap_or(0);
        if len > 200 {
            out[i].content = Some(format!("[output pruned: {len} chars]"));
            pruned += 1;
        }
    }
    while est_tokens(&out) > budget && out.len() > 6 {
        let Some(start) = out.iter().enumerate().position(|(i, m)| i >= 2 && m.role == "assistant") else { break };
        let mut end = start + 1;
        while end < out.len() && out[end].role == "tool" {
            end += 1;
        }
        if end + 2 >= out.len() {
            break;
        }
        out.drain(start..end);
        pruned += 1;
    }
    if pruned > 0 {
        let at = out.iter().enumerate().position(|(i, m)| i >= 1 && m.role != "system").unwrap_or(0);
        out.insert(at + 1, Message::user("[Earlier steps were compacted to fit context. Re-read files if you need exact content.]"));
    }
    (out, pruned)
}

fn transcript(msgs: &[Message]) -> String {
    let mut out = String::new();
    for m in msgs {
        match m.role.as_str() {
            "assistant" => {
                if let Some(c) = m.content.as_ref().filter(|c| !c.is_empty()) {
                    out.push_str(&format!("assistant: {}\n", c.chars().take(600).collect::<String>()));
                }
                for t in m.tool_calls.iter().flatten() {
                    out.push_str(&format!("call {}({})\n", t.function.name, t.function.arguments.chars().take(300).collect::<String>()));
                }
            }
            "tool" => out.push_str(&format!("result: {}\n", m.content.as_deref().unwrap_or("").chars().take(900).collect::<String>())),
            "user" if m.content.as_deref().map(|c| c.starts_with("[Earlier steps were compacted")).unwrap_or(false) => {}
            "user" => out.push_str(&format!("user: {}\n", m.content.as_deref().unwrap_or("").chars().take(900).collect::<String>())),
            _ => {}
        }
    }
    out
}

/// LLM compaction for when the history is over budget: the middle of the conversation (everything between the task
/// and the most recent steps) becomes one short summary produced with thinking off. None when nothing is worth
/// summarizing or the call fails (the deterministic pruning then applies as before).
pub async fn llm_compact(llm: &LlmClient, messages: &[Message], budget: usize, keep_tail: usize, cancel: Option<CancellationToken>) -> Option<Vec<Message>> {
    if est_tokens(messages) <= budget {
        return None;
    }
    let first_user = messages.iter().position(|m| m.role == "user")?;
    let head_end = first_user + 1;
    let mut tail_start = messages.len().saturating_sub(keep_tail).max(head_end);
    while tail_start < messages.len() && messages[tail_start].role != "assistant" {
        tail_start += 1;
    }
    if tail_start.saturating_sub(head_end) < 2 || tail_start >= messages.len() {
        return None;
    }
    let middle = &messages[head_end..tail_start];
    let prior = middle.first().filter(|m| m.content.as_deref().map(|c| c.starts_with(SUMMARY_TAG)).unwrap_or(false)).and_then(|m| m.content.clone());
    let body = format!("{}{}", prior.map(|p| format!("Previous summary:\n{p}\n\nNewer steps:\n")).unwrap_or_default(), transcript(middle));
    let r = llm
        .chat(ChatOptions {
            messages: vec![
                Message::system("You compress the working history of a coding agent so it can continue the task. Write a factual summary in at most 220 words: files read or changed (paths), key facts learned (names, values, error messages, constraints), decisions made, and what remains to do. No code blocks unless a short snippet is essential. Do not invent anything."),
                Message::user(body),
            ],
            thinking: Thinking::Off,
            max_tokens: Some(600),
            cancel,
            ..Default::default()
        })
        .await
        .ok()?;
    let text = r.content.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let mut out: Vec<Message> = messages[..head_end].to_vec();
    out.push(Message::user(format!("{SUMMARY_TAG}\n{text}\nRe-read files if you need exact content.")));
    out.extend_from_slice(&messages[tail_start..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{WireFunction, WireToolCall};

    #[test]
    fn prunes_old_tool_output_and_keeps_pairing() {
        let mut m = vec![Message::system("s"), Message::user("task")];
        for i in 0..20 {
            m.push(Message::assistant(None, vec![WireToolCall { id: format!("c{i}"), kind: "function".into(), function: WireFunction { name: "read_file".into(), arguments: "{}".into() } }]));
            m.push(Message::tool(format!("c{i}"), "x".repeat(4000)));
        }
        let before = est_tokens(&m);
        let (r, _) = compact(&m, 6000, 6);
        assert!(est_tokens(&r) < before / 2);
        for (i, x) in r.iter().enumerate() {
            if x.role == "tool" {
                assert!(r[i - 1].role == "assistant" || r[i - 1].role == "tool");
            }
        }
    }
}
