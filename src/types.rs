use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseState {
    /// arguments parsed as-is
    Ok,
    /// arguments needed repair
    Repaired,
    /// arguments unusable
    Malformed,
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub raw_args: String,
    pub args: Value,
    pub parse: ParseState,
    /// recovered from message content instead of tool_calls
    pub from_content: bool,
}

impl ToolCall {
    pub fn str_arg(&self, k: &str) -> Option<&str> {
        self.args.get(k).and_then(|v| v.as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireFunction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<WireToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(c: impl Into<String>) -> Self {
        Message { role: "system".into(), content: Some(c.into()), tool_calls: None, tool_call_id: None }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Message { role: "user".into(), content: Some(c.into()), tool_calls: None, tool_call_id: None }
    }
    pub fn assistant(c: Option<String>, calls: Vec<WireToolCall>) -> Self {
        Message { role: "assistant".into(), content: c, tool_calls: if calls.is_empty() { None } else { Some(calls) }, tool_call_id: None }
    }
    pub fn tool(id: impl Into<String>, c: impl Into<String>) -> Self {
        Message { role: "tool".into(), content: Some(c.into()), tool_calls: None, tool_call_id: Some(id.into()) }
    }
}

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl ToolSpec {
    pub fn to_wire(&self) -> Value {
        json!({"type": "function", "function": {"name": self.name, "description": self.description, "parameters": self.parameters}})
    }
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thinking {
    Off,
    Low,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Plan,
    Ask,
    AutoEdit,
    Yolo,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "plan" => Some(Mode::Plan),
            "ask" => Some(Mode::Ask),
            "auto-edit" => Some(Mode::AutoEdit),
            "yolo" => Some(Mode::Yolo),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LlmResult {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish: String,
    pub usage: Usage,
    pub ttft_ms: u64,
    pub total_ms: u64,
    pub repaired: u32,
    pub malformed: u32,
    pub think_leak: bool,
}
