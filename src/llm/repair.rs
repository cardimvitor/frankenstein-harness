//! Repair helpers for malformed tool-call JSON emitted by Qwen models.
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

static THINK_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<think>.*?(</think>|$)").unwrap());
static THINK_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</?think>").unwrap());
static TRAILING_COMMA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",\s*([}\]])").unwrap());
static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```(?:json)?\s*(.*?)```").unwrap());

pub struct ParseResult {
    pub ok: bool,
    pub value: Option<Value>,
    pub repaired: bool,
    pub think_leak: bool,
}

/// Remove `<think>` blocks; returns (text, whether any think tag was present).
pub fn strip_think(s: &str) -> (String, bool) {
    let leaked = THINK_TAG.is_match(s);
    let t = THINK_BLOCK.replace_all(s, "");
    (THINK_TAG.replace_all(&t, "").into_owned(), leaked)
}

fn try_parse(s: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(s) {
        Ok(v) if v.is_object() => Some(v),
        _ => None,
    }
}

fn strip_trailing_comma(s: &mut String) {
    let t = s.trim_end();
    if t.ends_with(',') {
        let n = t.len() - 1;
        s.truncate(n);
    }
}

/// Close unterminated strings/brackets and drop trailing commas.
pub fn close_json(s: &str) -> String {
    let mut stack: Vec<char> = Vec::new();
    let (mut in_str, mut esc) = (false, false);
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        out.push(ch);
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    if in_str {
        if esc {
            out.pop();
        }
        out.push('"');
    }
    strip_trailing_comma(&mut out);
    while let Some(c) = stack.pop() {
        strip_trailing_comma(&mut out);
        out.push(c);
    }
    out
}

pub fn parse_args(raw: &str) -> ParseResult {
    let (text, leaked) = strip_think(raw);
    let mut s = text.trim().to_string();
    if s.is_empty() {
        return ParseResult { ok: true, value: Some(Value::Object(Map::new())), repaired: false, think_leak: leaked };
    }
    if let Some(v) = try_parse(&s) {
        return ParseResult { ok: true, value: Some(v), repaired: leaked, think_leak: leaked };
    }
    if let Some(c) = FENCE.captures(&s) {
        s = c[1].trim().to_string();
    }
    if let Some(first) = s.find('{') {
        if first > 0 {
            s = s[first..].to_string();
        }
    }
    if let Some(last) = s.rfind('}') {
        if last > 0 && last < s.len() - 1 {
            s.truncate(last + 1);
        }
    }
    let nocomma = TRAILING_COMMA.replace_all(&s, "$1").into_owned();
    let mut v = try_parse(&s).or_else(|| try_parse(&nocomma)).or_else(|| try_parse(&close_json(&s))).or_else(|| try_parse(&close_json(&nocomma)));
    if v.is_none() && !s.contains('"') && s.contains('\'') {
        v = try_parse(&close_json(&s.replace('\'', "\"")));
    }
    match v {
        Some(v) => ParseResult { ok: true, value: Some(v), repaired: true, think_leak: leaked },
        None => ParseResult { ok: false, value: None, repaired: false, think_leak: leaked },
    }
}

#[derive(Debug, Clone)]
pub struct ContentCall {
    pub name: String,
    pub args: Value,
    pub raw: String,
}

fn parse_xml_function(body: &str) -> Option<ContentCall> {
    let start = body.find("<function=")?;
    let after = &body[start + "<function=".len()..];
    let gt = after.find('>')?;
    let name = after[..gt].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let inner_all = &after[gt + 1..];
    let inner = inner_all.split("</function>").next().unwrap_or(inner_all);
    let mut args = Map::new();
    let mut rest = inner;
    while let Some(p) = rest.find("<parameter=") {
        let a = &rest[p + "<parameter=".len()..];
        let Some(g) = a.find('>') else { break };
        let key = a[..g].trim().to_string();
        let val_all = &a[g + 1..];
        // value ends at </parameter>, the next <parameter=, or the end
        let end = [val_all.find("</parameter>"), val_all.find("<parameter=")].into_iter().flatten().min().unwrap_or(val_all.len());
        let mut val = val_all[..end].to_string();
        if val.starts_with('\n') {
            val.remove(0);
        }
        if val.ends_with('\n') {
            val.pop();
        }
        args.insert(key, Value::String(val));
        let skip = if val_all[end..].starts_with("</parameter>") { end + "</parameter>".len() } else { end };
        rest = &val_all[skip..];
    }
    Some(ContentCall { name, args: Value::Object(args), raw: String::new() })
}

/// Recover tool calls the server failed to parse and left in content:
///   <tool_call>{"name":..,"arguments":{..}}</tool_call>
///   <tool_call><function=name><parameter=k>v</parameter></function></tool_call>   (qwen3_coder XML)
pub fn extract_content_calls(content: &str) -> (Vec<ContentCall>, String) {
    let mut calls = Vec::new();
    let mut rest = content.to_string();
    let mut search_from = 0;
    while let Some(p) = content[search_from..].find("<tool_call>") {
        let start = search_from + p;
        let body_start = start + "<tool_call>".len();
        let (body, end) = match content[body_start..].find("</tool_call>") {
            Some(e) => (&content[body_start..body_start + e], body_start + e + "</tool_call>".len()),
            None => (&content[body_start..], content.len()),
        };
        let raw = &content[start..end];
        let body = body.trim();
        let mut call = parse_xml_function(body);
        if call.is_none() {
            let r = parse_args(body);
            if let Some(Value::Object(o)) = r.value {
                if let Some(Value::String(name)) = o.get("name") {
                    let a = o.get("arguments").or_else(|| o.get("parameters")).cloned().unwrap_or(Value::Object(Map::new()));
                    let args = match a {
                        Value::String(s) => parse_args(&s).value.unwrap_or(Value::Object(Map::new())),
                        v => v,
                    };
                    call = Some(ContentCall { name: name.clone(), args, raw: String::new() });
                }
            }
        }
        if let Some(mut c) = call {
            c.raw = raw.to_string();
            rest = rest.replacen(raw, "", 1);
            calls.push(c);
        }
        search_from = end;
    }
    (calls, rest.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_args_cases() {
        assert_eq!(parse_args(r#"{"a":1}"#).value.unwrap(), json!({"a":1}));
        assert!(!parse_args(r#"{"a":1}"#).repaired);
        assert_eq!(parse_args(r#"{"a":1,}"#).value.unwrap(), json!({"a":1}));
        assert_eq!(parse_args(r#"{"path":"x.ts","n":[1,2"#).value.unwrap(), json!({"path":"x.ts","n":[1,2]}));
        assert_eq!(parse_args(r#"{"s":"abc"#).value.unwrap(), json!({"s":"abc"}));
        assert_eq!(parse_args("```json\n{\"a\":2}\n```").value.unwrap(), json!({"a":2}));
        let t = parse_args(r#"<think>hmm</think>{"a":3}"#);
        assert_eq!(t.value.unwrap(), json!({"a":3}));
        assert!(t.think_leak);
        assert!(!parse_args("not json at all").ok);
        assert_eq!(close_json(r#"{"a":[1,2,"#), r#"{"a":[1,2]}"#);
        assert_eq!(parse_args("{'a': 'b'").value.unwrap(), json!({"a":"b"}));
        assert_eq!(parse_args("").value.unwrap(), json!({}));
    }

    #[test]
    fn content_calls_json_and_xml() {
        let (calls, rest) = extract_content_calls(r#"hi <tool_call>{"name":"read_file","arguments":{"path":"a"}}</tool_call>"#);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].args, json!({"path":"a"}));
        assert_eq!(rest, "hi");
        let (b, _) = extract_content_calls("<tool_call><function=edit><parameter=path>a.ts</parameter><parameter=old>x\ny</parameter></function></tool_call>");
        assert_eq!(b[0].name, "edit");
        assert_eq!(b[0].args, json!({"path":"a.ts","old":"x\ny"}));
        let (c, _) = extract_content_calls("<tool_call><function=bash><parameter=command>ls -la");
        assert_eq!(c[0].args, json!({"command":"ls -la"}));
    }

    #[test]
    fn strip_think_blocks() {
        let (t, l) = strip_think("a<think>x</think>b");
        assert_eq!(t, "ab");
        assert!(l);
        let (t, _) = strip_think("a<think>unterminated");
        assert_eq!(t, "a");
    }
}
