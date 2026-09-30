use fh::config::{load_config, Config, Env};
use fh::eval::corpus::builtin_tasks;
use fh::skills::store::SkillStore;
use fh::testkit::{self, Mock, Scripted};
use fh::ui::server::{start_web_server, ServeOptions, WebServer, CSP};
use futures_util::StreamExt;
use reqwest::Method;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

struct Env2 {
    m: Mock,
    d: tempfile::TempDir,
    srv: WebServer,
    cfg: Config,
}

async fn setup(env: Env) -> Env2 {
    let m = testkit::start(0, None).await;
    let d = tempfile::tempdir().unwrap();
    let t = builtin_tasks().into_iter().find(|t| t.id == "py-off-by-one").unwrap();
    for (f, b) in &t.files {
        std::fs::write(d.path().join(f), b).unwrap();
    }
    assert!(Command::new("sh").arg("-c").arg("git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm b").current_dir(d.path()).status().unwrap().success());
    let mut cfg = load_config(Path::new("/x"), &Env::new()).unwrap();
    cfg.endpoint = m.url.clone();
    cfg.metrics_url = format!("{}/metrics", m.base);
    cfg.retries = 0;
    cfg.model = "qwen-mock".into();
    let srv = start_web_server(cfg.clone(), env, d.path().to_path_buf(), ServeOptions { port: 0, store: Some(SkillStore::in_memory()), log: None }).await.unwrap();
    Env2 { m, d, srv, cfg }
}

struct R {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: String,
}

async fn http(port: u16, method: Method, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> R {
    let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let mut req = c.request(method, format!("http://127.0.0.1:{port}{path}"));
    let mut has_host = false;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("host") {
            has_host = true;
        }
        req = req.header(*k, *v);
    }
    if !has_host {
        req = req.header("host", format!("127.0.0.1:{port}"));
    }
    if let Some(b) = body {
        req = req.body(b.to_string());
    }
    let r = req.send().await.unwrap();
    R { status: r.status().as_u16(), headers: r.headers().clone(), body: r.text().await.unwrap_or_default() }
}

const J: [(&str, &str); 2] = [("content-type", "application/json"), ("x-fh", "1")];

async fn login(port: u16, code: &str) -> (R, String) {
    let r = http(port, Method::POST, "/api/auth", &J, Some(json!({"code": code}))).await;
    let cookie = r.headers.get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").to_string();
    (r, cookie)
}

#[tokio::test]
async fn static_pages_carry_strict_csp_and_no_inline_script_or_style() {
    let e = setup(Env::new()).await;
    let r = http(e.srv.port, Method::GET, "/", &[], None).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["content-security-policy"], CSP);
    assert!(CSP.contains("default-src 'none'") && !CSP.contains("unsafe-inline") && !CSP.contains("unsafe-eval") && !CSP.contains('*'));
    assert_eq!(r.headers["x-content-type-options"], "nosniff");
    assert_eq!(r.headers["referrer-policy"], "no-referrer");
    for tag in regex::Regex::new(r"<script[^>]*>").unwrap().find_iter(&r.body) {
        assert!(tag.as_str().contains("src="), "inline script tag: {}", tag.as_str());
    }
    assert!(!regex::Regex::new(r"\son\w+=|style=").unwrap().is_match(&r.body));
    assert!(e.srv.url.starts_with("http://127.0.0.1:") && !e.srv.url.contains("code") && !e.srv.url.contains('#'));
    let js = http(e.srv.port, Method::GET, "/app.js", &[], None).await.body;
    assert!(!regex::Regex::new(r"\.(innerHTML|outerHTML)\s*[+]?=|insertAdjacentHTML|document\.write|eval\(").unwrap().is_match(&js), "client must never inject markup");
}

#[tokio::test]
async fn host_origin_and_csrf_checks_and_no_cors() {
    let e = setup(Env::new()).await;
    let p = e.srv.port;
    assert_eq!(http(p, Method::GET, "/", &[("host", "evil.example.com")], None).await.status, 403);
    assert_eq!(http(p, Method::GET, "/", &[("host", &format!("evil.example.com:{p}"))], None).await.status, 403);
    assert_eq!(http(p, Method::GET, "/", &[("host", &format!("localhost:{p}"))], None).await.status, 200);
    let mut h = J.to_vec();
    h.push(("origin", "http://evil.example.com"));
    assert_eq!(http(p, Method::POST, "/api/auth", &h, Some(json!({"code": "x"}))).await.status, 403);
    assert_eq!(http(p, Method::POST, "/api/auth", &[("content-type", "text/plain")], Some(json!({}))).await.status, 403);
    let own = format!("http://127.0.0.1:{p}");
    let mut h2 = J.to_vec();
    h2.push(("origin", &own));
    let ok = http(p, Method::POST, "/api/auth", &h2, Some(json!({"code": "wrong"}))).await;
    assert_eq!(ok.status, 401);
    assert!(ok.headers.get("access-control-allow-origin").is_none());
    let pre = http(p, Method::OPTIONS, "/api/task", &[("origin", "http://evil.example.com"), ("access-control-request-method", "POST")], None).await;
    assert_eq!(pre.status, 403);
}

#[tokio::test]
async fn one_time_code_becomes_httponly_samesite_strict_cookie_and_is_single_use() {
    let e = setup(Env::new()).await;
    let p = e.srv.port;
    assert_eq!(http(p, Method::GET, "/api/state", &[], None).await.status, 401);
    let code = e.srv.code();
    let (r, cookie) = login(p, &code).await;
    assert_eq!(r.status, 200);
    let sc = r.headers["set-cookie"].to_str().unwrap().to_string();
    assert!(sc.contains("HttpOnly") && sc.contains("SameSite=Strict") && !sc.contains("Domain="));
    assert_eq!(http(p, Method::GET, "/api/state", &[("cookie", &cookie)], None).await.status, 200);
    assert_eq!(login(p, &code).await.0.status, 401); // consumed
    assert_ne!(e.srv.code(), code);
    assert_eq!(http(p, Method::GET, "/api/state", &[("cookie", "fh_session=forged")], None).await.status, 401);
}

#[tokio::test]
async fn repeated_bad_codes_rotate_the_code() {
    let e = setup(Env::new()).await;
    let c0 = e.srv.code();
    for i in 0..5 {
        http(e.srv.port, Method::POST, "/api/auth", &J, Some(json!({"code": format!("bad{i}")}))).await;
    }
    assert_ne!(e.srv.code(), c0);
    assert_eq!(login(e.srv.port, &c0).await.0.status, 401);
}

/// Collect SSE events until `stop` says so.
async fn sse(port: u16, cookie: &str, stop: impl Fn(&str, &Value) -> bool) -> Vec<(String, Value)> {
    let c = reqwest::Client::new();
    let res = c.get(format!("http://127.0.0.1:{port}/api/events")).header("host", format!("127.0.0.1:{port}")).header("cookie", cookie).send().await.unwrap();
    let mut s = res.bytes_stream();
    let (mut buf, mut out) = (String::new(), vec![]);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(deadline, s.next()).await {
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(i) = buf.find("\n\n") {
            let blk = buf[..i].to_string();
            buf.drain(..i + 2);
            let ty = blk.lines().find_map(|l| l.strip_prefix("event: "));
            let data = blk.lines().find_map(|l| l.strip_prefix("data: "));
            if let (Some(t), Some(d)) = (ty, data) {
                let v: Value = serde_json::from_str(d).unwrap_or(Value::Null);
                let done = stop(t, &v);
                out.push((t.to_string(), v));
                if done {
                    return out;
                }
            }
        }
    }
    out
}

#[tokio::test]
async fn end_to_end_task_over_the_api_with_sse_and_secret_redaction() {
    let mut env = Env::new();
    env.insert("FH_API_KEY".into(), "super-secret-token-value-123".into());
    let e = setup(env).await;
    e.m.set_fallback(|req| {
        let p = &req["response_format"]["json_schema"]["schema"]["properties"];
        if p.get("trivial").is_some() {
            return Scripted::json(json!({"trivial": true, "questions": [], "enriched": "fix sum_range", "acceptance": ["tests pass"], "plan": [{"step": "fix", "files": ["mathx.py"]}], "assumptions": [], "subtasks": []}));
        }
        if p.get("verdict").is_some() {
            return Scripted::json(json!({"verdict": "pass", "findings": []}));
        }
        if req["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" {
            return Scripted::text("Done. The key is super-secret-token-value-123 (should be redacted).");
        }
        Scripted::call("edit", json!({"path": "mathx.py", "old_text": "in range(a, b)", "new_text": "in range(a, b + 1)"}))
    });
    let p = e.srv.port;
    let (_, cookie) = login(p, &e.srv.code()).await;
    let listener = tokio::spawn({
        let cookie = cookie.clone();
        async move { sse(p, &cookie, |t, _| t == "result").await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut h = J.to_vec();
    h.push(("cookie", &cookie));
    let sub = http(p, Method::POST, "/api/task", &h, Some(json!({"task": "fix sum_range", "mode": "auto", "approval": "yolo"}))).await;
    assert_eq!(sub.status, 202);
    assert_eq!(http(p, Method::POST, "/api/task", &h, Some(json!({"task": "second"}))).await.status, 409); // one at a time
    let events = listener.await.unwrap();
    let result = &events.iter().find(|(t, _)| t == "result").expect("result event").1;
    assert_eq!(result["verdict"], "pass");
    assert!(result["diff"].as_str().unwrap().contains("b + 1"));
    assert!(events.iter().any(|(t, d)| t == "tool_end" && d["name"] == "edit"));
    assert!(events.iter().any(|(t, d)| t == "notice" && d["kind"] == "verify"));
    let all = serde_json::to_string(&events.iter().map(|(t, d)| json!([t, d])).collect::<Vec<_>>()).unwrap();
    assert!(!all.contains("super-secret-token-value-123"), "secret leaked into the event stream");
    assert!(std::fs::read_to_string(e.d.path().join("mathx.py")).unwrap().contains("b + 1"));
    let _ = &e.cfg;
}

#[tokio::test]
async fn interactive_plan_approval_round_trip_and_rejection() {
    let e = setup(Env::new()).await;
    e.m.set_fallback(|req| {
        if req["response_format"]["json_schema"]["schema"]["properties"].get("trivial").is_some() {
            return Scripted::json(json!({"trivial": false, "questions": [], "enriched": "x", "acceptance": [], "plan": [{"step": "do it", "files": ["mathx.py"]}], "assumptions": [], "subtasks": []}));
        }
        Scripted::text("noop")
    });
    let p = e.srv.port;
    let (_, cookie) = login(p, &e.srv.code()).await;
    let listener = tokio::spawn({
        let cookie = cookie.clone();
        async move { sse(p, &cookie, |t, _| t == "ask").await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut h = J.to_vec();
    h.push(("cookie", &cookie));
    http(p, Method::POST, "/api/task", &h, Some(json!({"task": "fix", "mode": "normal", "approval": "ask"}))).await;
    let events = listener.await.unwrap();
    let ask = &events.last().unwrap().1;
    assert_eq!(ask["kind"], "plan");
    assert!(ask["payload"]["plan"].as_str().unwrap().contains("do it"));
    assert_eq!(http(p, Method::POST, "/api/answer", &h, Some(json!({"id": "nope", "value": true}))).await.status, 404);
    http(p, Method::POST, "/api/answer", &h, Some(json!({"id": ask["id"], "value": {"ok": false}}))).await;
    for _ in 0..40 {
        let s: Value = serde_json::from_str(&http(p, Method::GET, "/api/state", &[("cookie", &cookie)], None).await.body).unwrap();
        if s["busy"] == false {
            assert_eq!(s["last"]["verdict"], "aborted");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("task did not finish");
}
