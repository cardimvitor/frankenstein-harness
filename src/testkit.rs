//! Mock vLLM (OpenAI-compatible, SSE, /metrics) used by the test suite and by `fh mock-server` for dry runs
//! of the VPS validation without a model.
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default, Debug)]
pub struct Scripted {
    pub content: Option<String>,
    pub reasoning: Option<String>,
    /// (name, arguments as a JSON value or raw string)
    pub tool_calls: Vec<(String, Value)>,
    pub status: Option<u16>,
    pub delay_ms: u64,
}

impl Scripted {
    pub fn text(s: impl Into<String>) -> Self {
        Scripted { content: Some(s.into()), ..Default::default() }
    }
    pub fn call(name: &str, args: Value) -> Self {
        Scripted { tool_calls: vec![(name.into(), args)], ..Default::default() }
    }
    pub fn json(v: Value) -> Self {
        Scripted::text(v.to_string())
    }
}

pub type Fallback = Arc<dyn Fn(&Value) -> Scripted + Send + Sync>;

#[derive(Default)]
pub struct MockState {
    pub queue: VecDeque<Scripted>,
    pub requests: Vec<Value>,
    pub fallback: Option<Fallback>,
    pub metrics_fn: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub api_key: Option<String>,
}

pub struct Mock {
    pub url: String,
    pub base: String,
    pub state: Arc<Mutex<MockState>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Mock {
    pub fn push(&self, s: Scripted) {
        self.state.lock().unwrap().queue.push_back(s);
    }
    pub fn set_fallback(&self, f: impl Fn(&Value) -> Scripted + Send + Sync + 'static) {
        self.state.lock().unwrap().fallback = Some(Arc::new(f));
    }
    pub fn set_metrics(&self, f: impl Fn() -> String + Send + Sync + 'static) {
        self.state.lock().unwrap().metrics_fn = Some(Arc::new(f));
    }
    pub fn requests(&self) -> Vec<Value> {
        self.state.lock().unwrap().requests.clone()
    }
    pub fn clear_requests(&self) {
        self.state.lock().unwrap().requests.clear();
    }
    pub fn stop(&mut self) {
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop();
    }
}

fn chunks(s: &str, n: usize) -> Vec<String> {
    let cs: Vec<char> = s.chars().collect();
    cs.chunks(n).map(|c| c.iter().collect()).collect()
}

fn sse(delta: Value, finish: Option<&str>) -> String {
    format!("data: {}\n\n", json!({"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}))
}

fn authorized(st: &MockState, h: &HeaderMap) -> bool {
    match &st.api_key {
        None => true,
        Some(k) => h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {k}")),
    }
}

async fn chat(State(state): State<Arc<Mutex<MockState>>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let scripted = {
        let mut st = state.lock().unwrap();
        if !authorized(&st, &headers) {
            return (StatusCode::UNAUTHORIZED, Json(json!({"error": {"message": "unauthorized"}}))).into_response();
        }
        st.requests.push(body.clone());
        match st.queue.pop_front() {
            Some(s) => s,
            None => match &st.fallback {
                Some(f) => f(&body),
                None => Scripted::text("ok"),
            },
        }
    };
    if scripted.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(scripted.delay_ms)).await;
    }
    if let Some(code) = scripted.status {
        return (StatusCode::from_u16(code).unwrap(), Json(json!({"error": {"message": "scripted"}}))).into_response();
    }
    let args_str = |v: &Value| if let Value::String(s) = v { s.clone() } else { v.to_string() };
    if body.get("stream") == Some(&Value::Bool(false)) {
        let tcs: Vec<Value> = scripted.tool_calls.iter().enumerate().map(|(i, (n, a))| json!({"id": format!("c{i}"), "type": "function", "function": {"name": n, "arguments": args_str(a)}})).collect();
        let msg = json!({"role": "assistant", "content": scripted.content, "tool_calls": if tcs.is_empty() { Value::Null } else { Value::Array(tcs.clone()) }});
        return Json(json!({"choices": [{"message": msg, "finish_reason": if tcs.is_empty() { "stop" } else { "tool_calls" }}], "usage": {"prompt_tokens": 100, "completion_tokens": 20}})).into_response();
    }
    let mut out = String::new();
    if let Some(r) = &scripted.reasoning {
        for p in chunks(r, 8) {
            out += &sse(json!({"reasoning_content": p}), None);
        }
    }
    if let Some(c) = &scripted.content {
        for p in chunks(c, 8) {
            out += &sse(json!({"content": p}), None);
        }
    }
    for (i, (n, a)) in scripted.tool_calls.iter().enumerate() {
        out += &sse(json!({"tool_calls": [{"index": i, "id": format!("c{i}"), "type": "function", "function": {"name": n, "arguments": ""}}]}), None);
        for p in chunks(&args_str(a), 6) {
            out += &sse(json!({"tool_calls": [{"index": i, "function": {"arguments": p}}]}), None);
        }
    }
    out += &sse(json!({}), Some(if scripted.tool_calls.is_empty() { "stop" } else { "tool_calls" }));
    out += &format!("data: {}\n\n", json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 20}}));
    out += "data: [DONE]\n\n";
    Response::builder().header(header::CONTENT_TYPE, "text/event-stream").body(Body::from(out)).unwrap()
}

async fn models(State(state): State<Arc<Mutex<MockState>>>, headers: HeaderMap) -> Response {
    if !authorized(&state.lock().unwrap(), &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({"data": [{"id": "mock-qwen", "max_model_len": 32768}]})).into_response()
}

async fn metrics(State(state): State<Arc<Mutex<MockState>>>) -> String {
    let f = state.lock().unwrap().metrics_fn.clone();
    f.map(|f| f()).unwrap_or_default()
}

/// Start a mock server on 127.0.0.1 (port 0 = any).
pub async fn start(port: u16, api_key: Option<&str>) -> Mock {
    let state = Arc::new(Mutex::new(MockState { api_key: api_key.map(|s| s.to_string()), ..Default::default() }));
    let app = Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/models", get(models))
        .route("/metrics", get(metrics))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("bind mock");
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).with_graceful_shutdown(async move { let _ = rx.await; }).await;
    });
    Mock { url: format!("http://{addr}/v1"), base: format!("http://{addr}"), state, shutdown: Some(tx) }
}

/// A scripted "model" that answers every probe of `fh validate-vllm` and solves the js-off-by-one eval task.
pub fn install_smart(mock: &Mock) {
    use std::sync::atomic::{AtomicU64, Ordering};
    let reqs = Arc::new(AtomicU64::new(0));
    let queries = Arc::new(AtomicU64::new(0));
    let hits = Arc::new(AtomicU64::new(0));
    let seen: Arc<Mutex<std::collections::HashSet<String>>> = Arc::new(Mutex::new(Default::default()));
    {
        let (r, q, h) = (reqs.clone(), queries.clone(), hits.clone());
        mock.set_metrics(move || {
            let n = r.load(Ordering::Relaxed);
            format!(
                "vllm:kv_cache_usage_perc 0.2\nvllm:num_requests_running 0\nvllm:num_requests_waiting 0\nvllm:prefix_cache_queries_total {}\nvllm:prefix_cache_hits_total {}\nvllm:spec_decode_num_drafts_total {}\nvllm:spec_decode_num_draft_tokens_total {}\nvllm:spec_decode_num_accepted_tokens_total {}\nvllm:spec_decode_num_accepted_tokens_per_pos_total{{position=\"0\"}} {}\nvllm:spec_decode_num_accepted_tokens_per_pos_total{{position=\"1\"}} {}\nvllm:spec_decode_num_accepted_tokens_per_pos_total{{position=\"2\"}} {}\n",
                q.load(Ordering::Relaxed), h.load(Ordering::Relaxed), n * 10, n * 30, n * 20, n * 9, n * 7, n * 4
            )
        });
    }
    mock.set_fallback(move |req| {
        reqs.fetch_add(1, Ordering::Relaxed);
        let msgs = req["messages"].as_array().cloned().unwrap_or_default();
        let text = msgs.iter().map(|m| m["content"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join("\n");
        let key: String = text.chars().take(200).collect();
        queries.fetch_add(100, Ordering::Relaxed);
        if !seen.lock().unwrap().insert(key) {
            hits.fetch_add(90, Ordering::Relaxed);
        }
        let props = &req["response_format"]["json_schema"]["schema"]["properties"];
        if props.get("trivial").is_some() {
            return Scripted::json(json!({"trivial": true, "questions": [], "enriched": "Fix sumRange to be inclusive", "acceptance": ["tests pass"], "plan": [{"step": "fix loop bound", "files": ["lib/math.js"]}], "assumptions": [], "subtasks": []}));
        }
        if props.get("verdict").is_some() {
            return Scripted::json(json!({"verdict": "pass", "findings": []}));
        }
        if props.get("create").is_some() {
            return Scripted::json(json!({"create": false}));
        }
        if let Some(cap) = regex::Regex::new(r"passphrase is (KX-\d+)").unwrap().captures(&text) {
            return Scripted::text(cap[1].to_string());
        }
        let last = msgs.last().cloned().unwrap_or(Value::Null);
        if last["role"] == "tool" {
            return Scripted::text("Fixed the loop bound.");
        }
        if text.contains("sumRange") && last["role"] == "user" {
            return Scripted::call("edit", json!({"path": "lib/math.js", "old_text": "i < b; i++", "new_text": "i <= b; i++"}));
        }
        if text.contains("Read the file") {
            return Scripted::call("read_file", json!({"path": "src/app.ts"}));
        }
        if text.contains("two-line snippet") {
            return Scripted::call("edit", json!({"path": "lib/util.js", "old_text": "const a = 1;\nconst b = \"two\";", "new_text": "const a = 10;\nconst b = \"three\"; // updated"}));
        }
        if text.contains("Run this exact shell") {
            return Scripted::call("bash", json!({"command": "grep -rn \"TODO(\\\"x\\\")\" src | head -5"}));
        }
        if text.contains("Create the new file") {
            return Scripted::call("write_file", json!({"path": "docs/notes.md", "content": "Olá, mundo"}));
        }
        if text.contains("Search the repo") {
            return Scripted::call("grep", json!({"pattern": "function\\s+\\w+\\("}));
        }
        if text.contains("Think carefully") {
            return Scripted { reasoning: Some("hmm".into()), tool_calls: vec![("read_file".into(), json!({"path": "src/parser.ts"}))], ..Default::default() };
        }
        Scripted::text("{\"items\":[]}")
    });
}
