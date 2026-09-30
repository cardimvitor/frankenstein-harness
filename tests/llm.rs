use fh::config::{load_config, Config, Env};
use fh::llm::client::{ChatOptions, LlmClient};
use fh::testkit::{self, Scripted};
use fh::types::*;
use serde_json::json;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn cfg_for(url: &str) -> Config {
    let mut c = load_config(Path::new("/nonexistent"), &Env::new()).unwrap();
    c.endpoint = url.to_string();
    c.metrics_url = url.replace("/v1", "/metrics");
    c.retries = 2;
    c
}
fn opts(text: &str) -> ChatOptions {
    ChatOptions { messages: vec![Message::user(text)], ..Default::default() }
}

#[tokio::test]
async fn streams_content_reasoning_and_tool_calls_and_sets_enable_thinking() {
    let m = testkit::start(0, None).await;
    m.push(Scripted { reasoning: Some("thinking here".into()), content: Some("hello world".into()), tool_calls: vec![("read_file".into(), json!({"path": "a.ts"}))], ..Default::default() });
    let c = LlmClient::new(cfg_for(&m.url), Env::new());
    let streamed = Arc::new(Mutex::new(String::new()));
    let s2 = streamed.clone();
    let mut o = opts("x");
    o.thinking = Thinking::High;
    o.on_content = Some(Arc::new(move |d| s2.lock().unwrap().push_str(d)));
    let r = c.chat(o).await.unwrap();
    assert_eq!(r.content, "hello world");
    assert_eq!(*streamed.lock().unwrap(), "hello world");
    assert_eq!(r.reasoning, "thinking here");
    assert_eq!(r.tool_calls[0].name, "read_file");
    assert_eq!(r.tool_calls[0].args, json!({"path": "a.ts"}));
    assert_eq!(r.tool_calls[0].parse, ParseState::Ok);
    let reqs = m.requests();
    assert_eq!(reqs[0]["chat_template_kwargs"]["enable_thinking"], true);
    assert_eq!(reqs[0]["top_k"], 20);
    let mut o2 = opts("y");
    o2.thinking = Thinking::Off;
    c.chat(o2).await.unwrap();
    assert_eq!(m.requests()[1]["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(c.stats().requests, 2);
}

#[tokio::test]
async fn repairs_malformed_streamed_args_and_counts_them() {
    let m = testkit::start(0, None).await;
    m.push(Scripted::call("bash", json!("{\"command\":\"ls -la\"")));
    let c = LlmClient::new(cfg_for(&m.url), Env::new());
    let r = c.chat(opts("x")).await.unwrap();
    assert_eq!(r.tool_calls[0].parse, ParseState::Repaired);
    assert_eq!(r.tool_calls[0].args, json!({"command": "ls -la"}));
    assert_eq!(c.stats().repaired, 1);
    assert_eq!(c.stats().malformed, 0);
}

#[tokio::test]
async fn recovers_tool_call_left_in_content() {
    let m = testkit::start(0, None).await;
    m.push(Scripted::text("<tool_call>{\"name\":\"grep\",\"arguments\":{\"pattern\":\"foo\"}}</tool_call>"));
    let c = LlmClient::new(cfg_for(&m.url), Env::new());
    let r = c.chat(opts("x")).await.unwrap();
    assert_eq!(r.tool_calls[0].name, "grep");
    assert!(r.tool_calls[0].from_content);
    assert_eq!(r.content, "");
}

#[tokio::test]
async fn retries_5xx_and_does_not_retry_401_and_sends_bearer() {
    let m = testkit::start(0, None).await;
    m.push(Scripted { status: Some(503), ..Default::default() });
    m.push(Scripted::text("fine"));
    let c = LlmClient::new(cfg_for(&m.url), Env::new());
    assert_eq!(c.chat(opts("x")).await.unwrap().content, "fine");
    assert_eq!(c.stats().retries, 1);

    let m2 = testkit::start(0, Some("secret-key-123")).await;
    let c2 = LlmClient::new(cfg_for(&m2.url), Env::new());
    let e = c2.chat(opts("x")).await.unwrap_err();
    assert!(e.msg.contains("401"), "{e}");
    assert_eq!(m2.requests().len(), 0);
    let mut env = Env::new();
    env.insert("FH_API_KEY".into(), "secret-key-123".into());
    let c3 = LlmClient::new(cfg_for(&m2.url), env);
    assert_eq!(c3.chat(opts("x")).await.unwrap().content, "ok");
}

#[tokio::test]
async fn abort_cancels_the_request() {
    let m = testkit::start(0, None).await;
    m.push(Scripted { delay_ms: 2000, content: Some("late".into()), ..Default::default() });
    let c = LlmClient::new(cfg_for(&m.url), Env::new());
    let tok = CancellationToken::new();
    let t2 = tok.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(50)).await; t2.cancel(); });
    let mut o = opts("x");
    o.cancel = Some(tok);
    let t0 = std::time::Instant::now();
    let e = c.chat(o).await.unwrap_err();
    assert!(e.aborted);
    assert!(t0.elapsed() < Duration::from_millis(1500));
}

#[tokio::test]
async fn json_falls_back_to_unconstrained_thinking_off_call() {
    let m = testkit::start(0, None).await;
    m.push(Scripted { reasoning: Some("long thoughts".into()), content: Some(String::new()), ..Default::default() });
    m.push(Scripted::text("{\"a\":1}"));
    let c = LlmClient::new(cfg_for(&m.url), Env::new());
    let mut o = opts("give json");
    o.thinking = Thinking::High;
    o.json_schema = Some(json!({"type": "object", "properties": {"a": {"type": "integer"}}}));
    let r = c.json(o).await.unwrap();
    assert_eq!(r.value.unwrap(), json!({"a": 1}));
    assert!(r.fallback);
    let reqs = m.requests();
    assert!(reqs[1].get("response_format").is_none());
    assert_eq!(reqs[1]["chat_template_kwargs"]["enable_thinking"], false);
    assert!(reqs[1]["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().contains("JSON schema"));
    m.push(Scripted::text("{\"b\":2}"));
    let r2 = c.json(opts("x")).await.unwrap();
    assert!(!r2.fallback);
    assert_eq!(r2.value.unwrap(), json!({"b": 2}));
}
