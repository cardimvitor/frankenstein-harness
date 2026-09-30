use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine as _;
use fh::config::Env;
use fh::mcp::oauth::{login, pkce_challenge, OAuthSession, OAuthStore};
use fh::mcp::{load_tools, McpClient, ServerConfig};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct S {
    base: String,
    challenge: Option<String>,
    valid: HashSet<String>,
    refresh_tokens: HashSet<String>,
    counter: u32,
    refreshes: u32,
    bad_state: bool,
    seen_resource: Vec<String>,
}

type St = Arc<Mutex<S>>;

async fn prm(State(s): State<St>) -> Json<Value> {
    let base = s.lock().unwrap().base.clone();
    Json(json!({"resource": format!("{base}/mcp"), "authorization_servers": [base]}))
}
async fn asm(State(s): State<St>) -> Json<Value> {
    let b = s.lock().unwrap().base.clone();
    Json(json!({"authorization_endpoint": format!("{b}/authorize"), "token_endpoint": format!("{b}/token"), "registration_endpoint": format!("{b}/register"), "code_challenge_methods_supported": ["S256"]}))
}
async fn register(Json(b): Json<Value>) -> Json<Value> {
    assert_eq!(b["token_endpoint_auth_method"], "none");
    assert!(b["redirect_uris"][0].as_str().unwrap().starts_with("http://127.0.0.1:"));
    Json(json!({"client_id": "cid-1"}))
}
async fn authorize(State(s): State<St>, Query(q): Query<HashMap<String, String>>) -> Response {
    let mut st = s.lock().unwrap();
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["client_id"], "cid-1");
    st.challenge = Some(q["code_challenge"].clone());
    st.seen_resource.push(q["resource"].clone());
    let state = if st.bad_state { "forged".to_string() } else { q["state"].clone() };
    Redirect::to(&format!("{}?code=CODE-1&state={state}", q["redirect_uri"])).into_response()
}
async fn token(State(s): State<St>, Form(f): Form<HashMap<String, String>>) -> Response {
    let mut st = s.lock().unwrap();
    st.counter += 1;
    let n = st.counter;
    match f["grant_type"].as_str() {
        "authorization_code" => {
            let ok_pkce = st.challenge.as_deref() == Some(pkce_challenge(&f["code_verifier"]).as_str());
            if f["code"] != "CODE-1" || !ok_pkce || f["resource"] != format!("{}/mcp", st.base) {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))).into_response();
            }
        }
        "refresh_token" => {
            if !st.refresh_tokens.remove(&f["refresh_token"]) {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))).into_response();
            }
            st.refreshes += 1;
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    }
    let (at, rt) = (format!("at-{n}"), format!("rt-{n}"));
    st.valid.insert(at.clone());
    st.refresh_tokens.insert(rt.clone());
    Json(json!({"access_token": at, "refresh_token": rt, "expires_in": 3600, "token_type": "Bearer"})).into_response()
}
async fn mcp(State(s): State<St>, headers: HeaderMap, Json(m): Json<Value>) -> Response {
    let (ok, base) = {
        let st = s.lock().unwrap();
        let tok = headers.get("authorization").and_then(|h| h.to_str().ok()).and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
        (st.valid.contains(tok), st.base.clone())
    };
    if !ok {
        return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, format!("Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\""))], "unauthorized").into_response();
    }
    let id = m.get("id").cloned();
    match (m["method"].as_str().unwrap_or(""), id) {
        (_, None) => StatusCode::ACCEPTED.into_response(),
        ("initialize", Some(id)) => Json(json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-03-26", "capabilities": {}}})).into_response(),
        ("tools/list", Some(id)) => Json(json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "whoami", "description": "d", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true}}]}})).into_response(),
        (_, Some(id)) => Json(json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "signed-in-user"}]}})).into_response(),
    }
}

async fn server() -> (String, St) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let st: St = Arc::new(Mutex::new(S { base: base.clone(), ..Default::default() }));
    let app = Router::new()
        .route("/.well-known/oauth-protected-resource", get(prm))
        .route("/.well-known/oauth-authorization-server", get(asm))
        .route("/register", post(register))
        .route("/authorize", get(authorize))
        .route("/token", post(token))
        .route("/mcp", post(mcp))
        .with_state(st.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (base, st)
}

/// Plays the browser: follows the authorize redirect to the loopback callback.
fn browser() -> Arc<dyn Fn(&str) + Send + Sync> {
    Arc::new(|url: &str| {
        let url = url.to_string();
        tokio::spawn(async move {
            let _ = reqwest::get(&url).await;
        });
    })
}

fn env_in(dir: &std::path::Path) -> Env {
    let mut e = Env::new();
    e.insert("FH_HOME".into(), dir.join("data").to_string_lossy().to_string());
    e.insert("FH_CONFIG_HOME".into(), dir.join("cfg").to_string_lossy().to_string());
    e
}

#[test]
fn pkce_challenge_matches_rfc7636_example() {
    // RFC 7636 appendix B
    assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    let _ = (Sha256::digest(b""), base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b""));
}

#[tokio::test]
async fn full_browser_flow_then_transparent_refresh_and_revocation_recovery() {
    let (base, st) = server().await;
    let d = tempfile::tempdir().unwrap();
    let env = env_in(d.path());
    let store = OAuthStore::new(d.path().join("data/mcp-oauth.json"));
    let url = format!("{base}/mcp");
    // before signing in the server answers 401 and the error says what to do
    let e = McpClient::connect("remote", &ServerConfig::Http { url: url.clone(), headers: Default::default() }, d.path()).await.err().unwrap();
    assert!(e.contains("401") && e.contains("fh mcp login remote"), "{e}");

    login("remote", &url, &Default::default(), &store, browser(), Duration::from_secs(20)).await.expect("login");
    let a = store.get("remote").unwrap();
    assert_eq!((a.client_id.as_str(), a.access_token.as_str(), a.refresh_token.as_deref()), ("cid-1", "at-1", Some("rt-1")));
    assert_eq!(st.lock().unwrap().seen_resource, vec![url.clone()], "the resource indicator names the MCP server");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.path().join("data/mcp-oauth.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }

    // configured like any other server: tools load with the stored token
    std::fs::create_dir_all(d.path().join("cfg")).unwrap();
    std::fs::write(d.path().join("cfg/mcp.json"), json!({"mcpServers": {"remote": {"url": url}}}).to_string()).unwrap();
    let (tools, notes) = load_tools(d.path(), &env).await;
    assert_eq!(tools.len(), 1, "{notes:?}");
    let ctx = fh::tools::ToolCtx::new(d.path());
    assert_eq!(tools[0].execute(&json!({}), &ctx).await.output, "signed-in-user");

    // an expiring token is refreshed before the request
    let mut a = store.get("remote").unwrap();
    a.expires_at = 1;
    store.put("remote", a).unwrap();
    assert_eq!(tools[0].execute(&json!({}), &ctx).await.output, "signed-in-user");
    assert_eq!(st.lock().unwrap().refreshes, 1);
    let a = store.get("remote").unwrap();
    assert_ne!(a.access_token, "at-1");
    assert!(a.expires_at > 1_000_000, "new expiry recorded");

    // the server revokes the token: the 401 triggers one refresh and the call still succeeds
    st.lock().unwrap().valid.clear();
    // (the refresh token stays valid, so a refresh yields a fresh access token)
    let r = tools[0].execute(&json!({}), &ctx).await;
    assert!(r.ok && r.output == "signed-in-user", "{}", r.output);
    assert_eq!(st.lock().unwrap().refreshes, 2);

    // when the refresh token is gone too the error tells the user to sign in again
    st.lock().unwrap().valid.clear();
    st.lock().unwrap().refresh_tokens.clear();
    let r = tools[0].execute(&json!({}), &ctx).await;
    assert!(!r.ok && r.output.contains("fh mcp login remote"), "{}", r.output);

    // logging out removes the stored tokens
    assert!(store.remove("remote").unwrap());
    assert!(OAuthSession::new("remote", store.clone()).is_none());
}

#[tokio::test]
async fn forged_state_and_abandoned_login_are_rejected() {
    let (base, st) = server().await;
    let d = tempfile::tempdir().unwrap();
    let store = OAuthStore::new(d.path().join("t.json"));
    st.lock().unwrap().bad_state = true;
    let e = login("r", &format!("{base}/mcp"), &Default::default(), &store, browser(), Duration::from_secs(20)).await.unwrap_err();
    assert!(e.contains("state mismatch"), "{e}");
    assert!(store.get("r").is_none());
    // nobody completes the browser step: the login gives up
    let e = login("r", &format!("{base}/mcp"), &Default::default(), &store, Arc::new(|_| {}), Duration::from_millis(600)).await.unwrap_err();
    assert!(e.contains("timed out"), "{e}");
}
