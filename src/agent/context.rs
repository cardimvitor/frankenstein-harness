use crate::types::Message;

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
