use crate::config::{auth_headers, redact, Config, Env};
use crate::llm::repair::{extract_content_calls, parse_args, strip_think};
use crate::types::*;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct LlmError {
    pub msg: String,
    pub retryable: bool,
    pub status: Option<u16>,
    pub aborted: bool,
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}
impl std::error::Error for LlmError {}

impl LlmError {
    fn new(msg: impl Into<String>, retryable: bool, status: Option<u16>) -> Self {
        LlmError { msg: msg.into(), retryable, status, aborted: false }
    }
    fn abort() -> Self {
        LlmError { msg: "aborted".into(), retryable: false, status: None, aborted: true }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LlmStats {
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub tool_calls: u64,
    pub repaired: u64,
    pub malformed: u64,
    pub think_leaks: u64,
    pub retries: u64,
    pub total_ms: u64,
}

pub type Callback = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct ChatOptions {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub thinking: Thinking,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub cancel: Option<CancellationToken>,
    /// JSON schema for structured output (response_format json_schema)
    pub json_schema: Option<Value>,
    pub on_content: Option<Callback>,
    pub on_reasoning: Option<Callback>,
    pub stop: Vec<String>,
}

impl Default for ChatOptions {
    fn default() -> Self {
        ChatOptions { messages: vec![], tools: vec![], thinking: Thinking::Off, max_tokens: None, temperature: None, cancel: None, json_schema: None, on_content: None, on_reasoning: None, stop: vec![] }
    }
}

pub struct JsonResult {
    pub value: Option<Value>,
    pub raw: LlmResult,
    pub fallback: bool,
}

/// OpenAI-compatible streaming client tuned for vLLM + Qwen (qwen3 reasoning parser, qwen3_coder tool parser).
#[derive(Clone)]
pub struct LlmClient {
    pub cfg: Config,
    pub env: Env,
    http: reqwest::Client,
    stats: Arc<Mutex<LlmStats>>,
}

impl LlmClient {
    pub fn new(cfg: Config, env: Env) -> Self {
        let http = reqwest::Client::builder().pool_idle_timeout(Duration::from_secs(30)).build().expect("http client");
        LlmClient { cfg, env, http, stats: Arc::new(Mutex::new(LlmStats::default())) }
    }

    pub fn stats(&self) -> LlmStats {
        self.stats.lock().unwrap().clone()
    }
    pub fn reset_stats(&self) {
        *self.stats.lock().unwrap() = LlmStats::default();
    }

    pub fn build_body(&self, o: &ChatOptions) -> Value {
        let s = if o.thinking == Thinking::Off { &self.cfg.sampling.instant } else { &self.cfg.sampling.thinking };
        let mut body = json!({
            "model": self.cfg.model,
            "messages": o.messages,
            "stream": true,
            "stream_options": {"include_usage": true},
            "max_tokens": o.max_tokens.unwrap_or(self.cfg.max_output_tokens),
            "temperature": o.temperature.unwrap_or(s.temperature),
            "top_p": s.top_p,
            "top_k": s.top_k,
            "chat_template_kwargs": {"enable_thinking": o.thinking != Thinking::Off},
        });
        let m = body.as_object_mut().unwrap();
        if let Some(p) = s.presence_penalty {
            m.insert("presence_penalty".into(), json!(p));
        }
        if !o.stop.is_empty() {
            m.insert("stop".into(), json!(o.stop));
        }
        if !o.tools.is_empty() {
            m.insert("tools".into(), Value::Array(o.tools.iter().map(|t| t.to_wire()).collect()));
            m.insert("tool_choice".into(), json!("auto"));
        }
        if let Some(schema) = &o.json_schema {
            m.insert("response_format".into(), json!({"type": "json_schema", "json_schema": {"name": "out", "schema": schema, "strict": true}}));
        }
        body
    }

    pub async fn chat(&self, o: ChatOptions) -> Result<LlmResult, LlmError> {
        let mut attempt = 0u32;
        loop {
            match self.once(&o).await {
                Ok(r) => return Ok(r),
                Err(e) => {
                    if e.aborted || o.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
                        return Err(LlmError::abort());
                    }
                    if !e.retryable || attempt >= self.cfg.retries {
                        return Err(e);
                    }
                    attempt += 1;
                    self.stats.lock().unwrap().retries += 1;
                    let backoff = (500u64 * 2u64.pow(attempt)).min(8000) + rand::random::<u64>() % 200;
                    let sleep = tokio::time::sleep(Duration::from_millis(backoff));
                    match &o.cancel {
                        Some(c) => tokio::select! { _ = sleep => {}, _ = c.cancelled() => return Err(LlmError::abort()) },
                        None => sleep.await,
                    }
                }
            }
        }
    }

    async fn once(&self, o: &ChatOptions) -> Result<LlmResult, LlmError> {
        let t0 = Instant::now();
        let never = CancellationToken::new();
        let cancel = o.cancel.as_ref().unwrap_or(&never);
        let mut req = self.http.post(format!("{}/chat/completions", self.cfg.endpoint)).header("content-type", "application/json").header("accept", "text/event-stream");
        for (k, v) in auth_headers(&self.cfg, &self.env) {
            req = req.header(k, v);
        }
        let req = req.body(self.build_body(o).to_string());
        let idle = Duration::from_millis(self.cfg.idle_timeout_ms);
        let overall = Duration::from_millis(self.cfg.request_timeout_ms);

        let res = tokio::select! {
            r = tokio::time::timeout(idle, req.send()) => match r {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return Err(LlmError::new(format!("network error: {}", redact(&e.to_string(), &self.env)), true, None)),
                Err(_) => return Err(LlmError::new("stream idle timeout", true, None)),
            },
            _ = cancel.cancelled() => return Err(LlmError::abort()),
        };
        if !res.status().is_success() {
            let code = res.status().as_u16();
            let txt = redact(&res.text().await.unwrap_or_default(), &self.env);
            let cut: String = txt.chars().take(500).collect();
            return Err(LlmError::new(format!("HTTP {code}: {cut}"), code == 429 || code >= 500, Some(code)));
        }
        tokio::select! {
            r = tokio::time::timeout(overall, self.consume(res, o, t0, idle)) => match r {
                Ok(r) => r,
                Err(_) => Err(LlmError::new("request timeout", true, None)),
            },
            _ = cancel.cancelled() => Err(LlmError::abort()),
        }
    }

    async fn consume(&self, res: reqwest::Response, o: &ChatOptions, t0: Instant, idle: Duration) -> Result<LlmResult, LlmError> {
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut finish = String::new();
        let mut ttft: Option<u64> = None;
        let mut usage = Usage::default();
        let mut acc: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
        let mut buf = String::new();
        let mut stream = res.bytes_stream();
        let mut pending: Vec<u8> = Vec::new();

        let handle = |line: &str, content: &mut String, reasoning: &mut String, finish: &mut String, ttft: &mut Option<u64>, usage: &mut Usage, acc: &mut BTreeMap<usize, (String, String, String)>| -> Result<(), LlmError> {
            let Some(data) = line.strip_prefix("data:") else { return Ok(()) };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                return Ok(());
            }
            let Ok(j) = serde_json::from_str::<Value>(data) else { return Ok(()) };
            if let Some(err) = j.get("error") {
                let m = err.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()).unwrap_or_else(|| err.to_string());
                return Err(LlmError::new(m, false, None));
            }
            if let Some(u) = j.get("usage").filter(|u| u.is_object()) {
                usage.prompt_tokens = u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                usage.completion_tokens = u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                usage.cached_tokens = u.pointer("/prompt_tokens_details/cached_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            }
            let Some(ch) = j.pointer("/choices/0") else { return Ok(()) };
            let d = ch.get("delta").cloned().unwrap_or(Value::Null);
            let mark = |ttft: &mut Option<u64>| {
                if ttft.is_none() {
                    *ttft = Some(t0.elapsed().as_millis() as u64);
                }
            };
            let rs = d.get("reasoning_content").or_else(|| d.get("reasoning")).and_then(|x| x.as_str());
            if let Some(rs) = rs.filter(|s| !s.is_empty()) {
                mark(ttft);
                reasoning.push_str(rs);
                if let Some(cb) = &o.on_reasoning {
                    cb(rs);
                }
            }
            if let Some(c) = d.get("content").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
                mark(ttft);
                content.push_str(c);
                if let Some(cb) = &o.on_content {
                    cb(c);
                }
            }
            if let Some(tcs) = d.get("tool_calls").and_then(|x| x.as_array()) {
                for tc in tcs {
                    mark(ttft);
                    let idx = tc.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                    let cur = acc.entry(idx).or_default();
                    if let Some(id) = tc.get("id").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
                        cur.0 = id.to_string();
                    }
                    if let Some(n) = tc.pointer("/function/name").and_then(|x| x.as_str()) {
                        cur.1.push_str(n);
                    }
                    if let Some(a) = tc.pointer("/function/arguments").and_then(|x| x.as_str()) {
                        cur.2.push_str(a);
                    }
                }
            }
            if let Some(f) = ch.get("finish_reason").and_then(|x| x.as_str()) {
                *finish = f.to_string();
            }
            Ok(())
        };

        loop {
            let next = tokio::time::timeout(idle, stream.next()).await.map_err(|_| LlmError::new("stream idle timeout", true, None))?;
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|e| LlmError::new(format!("network error: {}", redact(&e.to_string(), &self.env)), true, None))?;
            pending.extend_from_slice(&chunk);
            // decode only complete UTF-8 (a multi-byte char may straddle chunks)
            let valid = match std::str::from_utf8(&pending) {
                Ok(s) => s.len(),
                Err(e) => e.valid_up_to(),
            };
            buf.push_str(std::str::from_utf8(&pending[..valid]).unwrap());
            pending.drain(..valid);
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim_end_matches('\r').to_string();
                buf.drain(..=nl);
                handle(&line, &mut content, &mut reasoning, &mut finish, &mut ttft, &mut usage, &mut acc)?;
            }
        }
        if !buf.trim().is_empty() {
            let l = buf.trim().to_string();
            handle(&l, &mut content, &mut reasoning, &mut finish, &mut ttft, &mut usage, &mut acc)?;
        }

        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let (mut repaired, mut malformed) = (0u32, 0u32);
        let mut think_leak = false;
        let mut n = 0;
        for (_, (id, name, args)) in acc {
            if name.is_empty() {
                continue;
            }
            let p = parse_args(&args);
            think_leak |= p.think_leak;
            if p.repaired {
                repaired += 1;
            }
            if !p.ok {
                malformed += 1;
            }
            let state = if !p.ok { ParseState::Malformed } else if p.repaired { ParseState::Repaired } else { ParseState::Ok };
            tool_calls.push(ToolCall {
                id: if id.is_empty() { n += 1; format!("call_{}_{}", t0.elapsed().as_nanos(), n) } else { id },
                name: strip_think(&name).0.trim().to_string(),
                raw_args: args,
                args: p.value.unwrap_or(Value::Object(Default::default())),
                parse: state,
                from_content: false,
            });
        }
        // recover tool calls a failed server-side parser left in content
        if tool_calls.is_empty() && content.contains("<tool_call>") {
            let (calls, rest) = extract_content_calls(&content);
            if !calls.is_empty() {
                content = rest;
                repaired += calls.len() as u32;
                for c in calls {
                    n += 1;
                    tool_calls.push(ToolCall { id: format!("call_c{}_{}", t0.elapsed().as_nanos(), n), name: c.name, raw_args: c.args.to_string(), args: c.args, parse: ParseState::Repaired, from_content: true });
                }
            }
        }
        // reasoning that leaked into visible content (no reasoning parser configured)
        let (stripped, leaked) = strip_think(&content);
        if leaked {
            think_leak = true;
            if let Some(m) = regex::Regex::new(r"(?is)<think>(.*?)(</think>|$)").unwrap().captures(&content) {
                reasoning.push_str(&m[1]);
            }
            content = stripped.trim().to_string();
        }

        let total_ms = t0.elapsed().as_millis() as u64;
        {
            let mut s = self.stats.lock().unwrap();
            s.requests += 1;
            s.prompt_tokens += usage.prompt_tokens;
            s.completion_tokens += usage.completion_tokens;
            s.cached_tokens += usage.cached_tokens;
            s.tool_calls += tool_calls.len() as u64;
            s.repaired += repaired as u64;
            s.malformed += malformed as u64;
            if think_leak {
                s.think_leaks += 1;
            }
            s.total_ms += total_ms;
        }
        Ok(LlmResult { content, reasoning, tool_calls, finish, usage, ttft_ms: ttft.unwrap_or(total_ms), total_ms, repaired, malformed, think_leak })
    }

    /// Structured (JSON) call, parsed with repair. Some vLLM versions apply guided decoding from the first token, which
    /// fights the reasoning block; if the constrained call yields no usable JSON, retry once without the schema and with
    /// thinking off, asking for JSON in the prompt.
    pub async fn json(&self, o: ChatOptions) -> Result<JsonResult, LlmError> {
        let parse = |t: &str| if t.trim().is_empty() { None } else { let p = parse_args(t); if p.ok { p.value } else { None } };
        let raw = self.chat(o.clone()).await?;
        let v = parse(&raw.content);
        if v.is_none() && (o.json_schema.is_some() || o.thinking != Thinking::Off) {
            let hint = match &o.json_schema {
                Some(s) => format!("\n\nReply with ONLY a JSON object matching this JSON schema, no prose and no code fences:\n{s}"),
                None => "\n\nReply with ONLY a JSON object, no prose.".to_string(),
            };
            let mut msgs = o.messages.clone();
            if let Some(last) = msgs.last_mut() {
                if last.role == "user" {
                    last.content = Some(format!("{}{}", last.content.clone().unwrap_or_default(), hint));
                }
            }
            let mut o2 = o.clone();
            o2.messages = msgs;
            o2.json_schema = None;
            o2.thinking = Thinking::Off;
            let raw2 = self.chat(o2).await?;
            let v2 = parse(&raw2.content);
            return Ok(JsonResult { value: v2, raw: raw2, fallback: true });
        }
        Ok(JsonResult { value: v, raw, fallback: false })
    }
}
