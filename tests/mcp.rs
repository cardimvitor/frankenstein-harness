use fh::agent::permissions::{decide, Decision};
use fh::config::Env;
use fh::mcp::{load_tools, McpClient, ServerConfig};
use fh::tools::ToolCtx;
use fh::types::Mode;
use serde_json::json;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn script() -> String {
    format!("{}/tests/fixtures/mcp_server.py", env!("CARGO_MANIFEST_DIR"))
}

fn stdio_cfg() -> ServerConfig {
    ServerConfig::Stdio { command: "python3".into(), args: vec![script()], env: HashMap::new(), cwd: None }
}

#[tokio::test]
async fn stdio_list_call_error_timeout_and_cancel() {
    let d = tempfile::tempdir().unwrap();
    let c = McpClient::connect("mock", &stdio_cfg(), d.path()).await.unwrap();
    let tools = c.list_tools().await.unwrap();
    assert_eq!(tools.len(), 4);
    assert!(tools.iter().find(|t| t.name == "echo").unwrap().read_only);
    assert!(!tools.iter().find(|t| t.name == "write").unwrap().read_only);
    let r = c.request("tools/call", json!({"name": "echo", "arguments": {"text": "hi"}}), Duration::from_secs(5), None).await.unwrap();
    assert_eq!(r["content"][0]["text"], "echo:hi");
    let e = c.request("tools/call", json!({"name": "nope"}), Duration::from_secs(5), None).await.unwrap_err();
    assert!(e.contains("unknown tool"));
    let t0 = Instant::now();
    let e = c.request("tools/call", json!({"name": "slow"}), Duration::from_millis(400), None).await.unwrap_err();
    assert!(e.contains("timed out") && t0.elapsed() < Duration::from_secs(3), "{e}");
    let tok = CancellationToken::new();
    let t2 = tok.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(200)).await; t2.cancel(); });
    let e = c.request("tools/call", json!({"name": "slow"}), Duration::from_secs(20), Some(&tok)).await.unwrap_err();
    assert_eq!(e, "cancelled");
}

#[tokio::test]
async fn config_load_tools_trust_gating_and_permissions() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let mut env = Env::new();
    env.insert("FH_HOME".into(), home.path().to_string_lossy().to_string());
    env.insert("FH_CONFIG_HOME".into(), home.path().join("cfg").to_string_lossy().to_string());
    std::fs::create_dir_all(ws.path().join(".fh")).unwrap();
    std::fs::write(ws.path().join(".fh/mcp.json"), json!({"mcpServers": {"my.srv": {"command": "python3", "args": [script()]}}}).to_string()).unwrap();
    let (tools, notes) = load_tools(ws.path(), &env).await;
    assert!(tools.is_empty() && notes[0].contains("not trusted"), "{notes:?}");
    fh::trust::trust(&env, ws.path()).unwrap();
    let (tools, _) = load_tools(ws.path(), &env).await;
    let names: Vec<String> = tools.iter().map(|t| t.spec().name.clone()).collect();
    assert!(names.contains(&"mcp__my_srv__echo".to_string()), "{names:?}");
    let echo = tools.iter().find(|t| t.spec().name.ends_with("__echo")).unwrap();
    let write = tools.iter().find(|t| t.spec().name.ends_with("__write")).unwrap();
    let boom = tools.iter().find(|t| t.spec().name.ends_with("__boom")).unwrap();
    let ctx = ToolCtx::new(ws.path());
    let r = echo.execute(&json!({"text": "yo"}), &ctx).await;
    assert!(r.ok && r.output == "echo:yo");
    let r = boom.execute(&json!({}), &ctx).await;
    assert!(!r.ok && r.output == "it broke");
    // read-only tool allowed in plan mode, mutating one is not
    assert!(matches!(decide(Mode::Plan, echo.as_ref(), &json!({})), Decision::Allow));
    assert!(matches!(decide(Mode::Plan, write.as_ref(), &json!({})), Decision::Deny { ask: false, .. }));
    assert!(matches!(decide(Mode::Ask, write.as_ref(), &json!({})), Decision::Deny { ask: true, .. }));
}

#[tokio::test]
async fn streamable_http_transport_json_and_sse() {
    use axum::{extract::State, http::HeaderMap, response::IntoResponse, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    async fn h(State(seen): State<Arc<Mutex<Vec<String>>>>, headers: HeaderMap, Json(m): Json<serde_json::Value>) -> axum::response::Response {
        seen.lock().unwrap().push(format!("{}|{}|{}", m["method"].as_str().unwrap_or(""), headers.get("authorization").and_then(|h| h.to_str().ok()).unwrap_or(""), headers.get("mcp-session-id").and_then(|h| h.to_str().ok()).unwrap_or("")));
        let id = m.get("id").cloned();
        match (m["method"].as_str().unwrap_or(""), id) {
            (_, None) => axum::http::StatusCode::ACCEPTED.into_response(),
            ("initialize", Some(id)) => ([("mcp-session-id", "S1")], Json(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-03-26","capabilities":{}}}))).into_response(),
            ("tools/list", Some(id)) => Json(json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"ping","description":"p","inputSchema":{"type":"object"}}]}})).into_response(),
            (_, Some(id)) => ([("content-type", "text/event-stream")], format!("event: message\ndata: {}\n\n", json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"pong"}]}}))).into_response(),
        }
    }
    let app = Router::new().route("/mcp", post(h)).with_state(seen.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap(); });
    let d = tempfile::tempdir().unwrap();
    let cfg = ServerConfig::Http { url: format!("http://{addr}/mcp"), headers: [("Authorization".to_string(), "Bearer T".to_string())].into() };
    let c = McpClient::connect("h", &cfg, d.path()).await.unwrap();
    assert_eq!(c.list_tools().await.unwrap()[0].name, "ping");
    let r = c.request("tools/call", json!({"name": "ping"}), Duration::from_secs(5), None).await.unwrap();
    assert_eq!(r["content"][0]["text"], "pong");
    let s = seen.lock().unwrap().clone();
    assert!(s.iter().any(|x| x.starts_with("tools/list|Bearer T|S1")), "{s:?}");
}
