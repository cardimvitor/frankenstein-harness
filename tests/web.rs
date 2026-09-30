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

#[tokio::test]
async fn tree_and_history_endpoints_need_auth_and_return_workspace_data() {
    let e = setup(Env::new()).await;
    let p = e.srv.port;
    assert_eq!(http(p, Method::GET, "/api/tree", &[], None).await.status, 401);
    assert_eq!(http(p, Method::GET, "/api/history", &[], None).await.status, 401);
    let (_, cookie) = login(p, &e.srv.code()).await;
    let t: Value = serde_json::from_str(&http(p, Method::GET, "/api/tree", &[("cookie", &cookie)], None).await.body).unwrap();
    assert!(t["files"].as_array().unwrap().iter().any(|f| f == "mathx.py"));
    assert!(!t["files"].as_array().unwrap().iter().any(|f| f.as_str().unwrap().starts_with(".git/")));
    let h: Value = serde_json::from_str(&http(p, Method::GET, "/api/history", &[("cookie", &cookie)], None).await.body).unwrap();
    assert!(h.is_array());
}

#[tokio::test]
async fn render_module_is_served_and_free_of_markup_injection() {
    let e = setup(Env::new()).await;
    let r = http(e.srv.port, Method::GET, "/render.js", &[], None).await;
    assert_eq!(r.status, 200);
    assert!(r.headers["content-type"].to_str().unwrap().starts_with("text/javascript"));
    assert!(!regex::Regex::new(r"\.(innerHTML|outerHTML)\s*[+]?=|insertAdjacentHTML|document\.write|eval\(|new Function|createElement\(.script.\)|\.href\s*=").unwrap().is_match(&r.body), "render.js must never inject markup or create links");
}

fn env_home(dir: &Path) -> Env {
    let mut e = Env::new();
    e.insert("FH_HOME".into(), dir.to_string_lossy().to_string());
    e.insert("FH_API_KEY".into(), "super-secret-token-value-123".into());
    e
}

#[tokio::test]
async fn file_endpoint_serves_workspace_files_and_refuses_everything_else() {
    let home = tempfile::tempdir().unwrap();
    let e = setup(env_home(home.path())).await;
    let p = e.srv.port;
    std::fs::write(e.d.path().join("notes.txt"), "token is super-secret-token-value-123 ok\n").unwrap();
    std::fs::write(e.d.path().join(".env"), "SECRET=1\n").unwrap();
    std::fs::write(e.d.path().join("server.pem"), "-----BEGIN-----\n").unwrap();
    std::fs::write(e.d.path().join("blob.bin"), [0u8, 1, 2, 3, 0]).unwrap();
    std::fs::write(e.d.path().join("big.txt"), "x".repeat(300_000)).unwrap();
    assert_eq!(http(p, Method::GET, "/api/file?path=mathx.py", &[], None).await.status, 401);
    let (_, cookie) = login(p, &e.srv.code()).await;
    let get = |path: String| {
        let cookie = cookie.clone();
        async move { http(p, Method::GET, &format!("/api/file?path={path}"), &[("cookie", &cookie)], None).await }
    };
    let ok = get("mathx.py".into()).await;
    let v: Value = serde_json::from_str(&ok.body).unwrap();
    assert!(ok.status == 200 && v["content"].as_str().unwrap().contains("def sum_range") && v["binary"] == false, "{}", ok.body);
    let n: Value = serde_json::from_str(&get("notes.txt".into()).await.body).unwrap();
    assert!(!n["content"].as_str().unwrap().contains("super-secret-token-value-123"), "secret must be redacted in served files");
    for bad in ["../../etc/passwd", "%2e%2e/%2e%2e/etc/passwd", ".git/config", ".env", "server.pem", "/etc/passwd"] {
        let r = get(bad.into()).await;
        assert_eq!(r.status, 403, "{bad} should be refused: {} {}", r.status, r.body);
    }
    assert_eq!(get("nope.txt".into()).await.status, 404);
    let b: Value = serde_json::from_str(&get("blob.bin".into()).await.body).unwrap();
    assert_eq!((b["binary"].clone(), b.get("content").is_none()), (json!(true), true));
    let big: Value = serde_json::from_str(&get("big.txt".into()).await.body).unwrap();
    assert!(big["truncated"] == true && big["content"].as_str().unwrap().len() == 200_000 && big["size"] == 300_000);
}

#[tokio::test]
async fn sessions_list_and_resume_endpoints() {
    let home = tempfile::tempdir().unwrap();
    let env = env_home(home.path());
    let e = setup(env.clone()).await;
    let p = e.srv.port;
    let fp = fh::fingerprint::fingerprint(e.d.path());
    // one finished session and one interrupted after planning
    let done = fh::session::log::SessionLog::create(&env, &fp.project_id);
    done.append(json!({"t": "start", "task": "old finished task", "auto": true}));
    done.append(json!({"t": "result", "verdict": "pass", "reason": "passed in round 1"}));
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let cut = fh::session::log::SessionLog::create(&env, &fp.project_id);
    cut.append(json!({"t": "start", "task": "fix sum_range super-secret-token-value-123", "auto": true}));
    let plan = json!({"trivial": true, "questions": [], "enriched": "fix sum_range", "acceptance": ["tests pass"], "plan": [{"step": "fix", "files": ["mathx.py"]}], "assumptions": [], "subtasks": []});
    cut.append(json!({"t": "plan", "plan": plan}));
    let base = fh::session::checkpoint::Checkpoints::new(e.d.path()).create("before").await.unwrap();
    cut.append(json!({"t": "checkpoint", "id": base}));

    assert_eq!(http(p, Method::GET, "/api/sessions", &[], None).await.status, 401);
    let (_, cookie) = login(p, &e.srv.code()).await;
    let list: Value = serde_json::from_str(&http(p, Method::GET, "/api/sessions", &[("cookie", &cookie)], None).await.body).unwrap();
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0]["resumable"].clone(), rows[0]["verdict"].clone()), (json!(true), Value::Null), "newest first: the interrupted one");
    assert!(!rows[0]["task"].as_str().unwrap().contains("super-secret-token-value-123"));
    assert_eq!(rows[1]["verdict"], "pass");

    let mut h = J.to_vec();
    h.push(("cookie", &cookie));
    let r = http(p, Method::POST, "/api/resume", &h, Some(json!({"id": done.id}))).await;
    assert_eq!(r.status, 400);
    assert!(r.body.contains("already finished"), "{}", r.body);
    assert_eq!(http(p, Method::POST, "/api/resume", &h, Some(json!({"id": "0000000000000"}))).await.status, 400);

    // resuming the interrupted one runs the stored plan without a new planning request
    e.m.set_fallback(|req| {
        let props = &req["response_format"]["json_schema"]["schema"]["properties"];
        assert!(props.get("trivial").is_none(), "resume must not re-plan");
        if props.get("verdict").is_some() {
            return Scripted::json(json!({"verdict": "pass", "findings": []}));
        }
        if req["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" {
            return Scripted::text("Fixed.");
        }
        Scripted::call("edit", json!({"path": "mathx.py", "old_text": "in range(a, b)", "new_text": "in range(a, b + 1)"}))
    });
    let listener = tokio::spawn({
        let cookie = cookie.clone();
        async move { sse(p, &cookie, |t, _| t == "result").await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(http(p, Method::POST, "/api/resume", &h, Some(json!({"id": cut.id, "mode": "auto", "approval": "yolo"}))).await.status, 202);
    let events = listener.await.unwrap();
    assert_eq!(events.iter().find(|(t, _)| t == "result").expect("result").1["verdict"], "pass");
    assert!(std::fs::read_to_string(e.d.path().join("mathx.py")).unwrap().contains("b + 1"));
}

#[tokio::test]
async fn search_endpoint_finds_files_lines_and_past_tasks_without_leaking_withheld_files() {
    let home = tempfile::tempdir().unwrap();
    let env = env_home(home.path());
    let e = setup(env.clone()).await;
    let p = e.srv.port;
    std::fs::create_dir_all(e.d.path().join("src")).unwrap();
    std::fs::write(e.d.path().join("src/orders.py"), "def list_orders():\n    return []\n\n# TODO paginate Orders\n").unwrap();
    std::fs::write(e.d.path().join(".env"), "ORDERS_SECRET=abc\n").unwrap();
    std::fs::write(e.d.path().join("notes.txt"), "token super-secret-token-value-123 for orders\n").unwrap();
    std::fs::write(e.d.path().join("blob.bin"), b"orders\0\0orders").unwrap();
    assert_eq!(http(p, Method::GET, "/api/search?q=orders", &[], None).await.status, 401);
    let (_, cookie) = login(p, &e.srv.code()).await;
    let get = |qs: String| {
        let cookie = cookie.clone();
        async move { http(p, Method::GET, &format!("/api/search?{qs}"), &[("cookie", &cookie)], None).await }
    };
    assert_eq!(get("q=".into()).await.status, 400);
    let r: Value = serde_json::from_str(&get("q=orders".into()).await.body).unwrap();
    // the file named orders.py ranks first among names
    assert_eq!(r["names"][0], "src/orders.py");
    let lines: Vec<(String, u64)> = r["matches"].as_array().unwrap().iter().map(|m| (m["path"].as_str().unwrap().to_string(), m["line"].as_u64().unwrap())).collect();
    assert!(lines.contains(&("src/orders.py".to_string(), 1)) && lines.contains(&("src/orders.py".to_string(), 4)), "{lines:?}");
    // case-insensitive, and neither the .env file nor the binary file is searched; secrets are redacted in the text
    assert!(!lines.iter().any(|(p, _)| p == ".env" || p == "blob.bin"), "{lines:?}");
    assert!(!r.to_string().contains("super-secret-token-value-123"));
    // past tasks
    let fp = fh::fingerprint::fingerprint(e.d.path());
    let l = fh::session::log::SessionLog::create(&env, &fp.project_id);
    l.append(json!({"t": "start", "task": "paginate the orders list", "auto": true}));
    l.append(json!({"t": "result", "verdict": "pass", "reason": "ok", "changed": ["src/orders.py"], "final": "Added pagination to list_orders."}));
    let s: Value = serde_json::from_str(&get("kind=sessions&q=pagination".into()).await.body).unwrap();
    assert_eq!(s["sessions"].as_array().unwrap().len(), 1, "{s}");
    assert_eq!(s["sessions"][0]["verdict"], "pass");
    // a match cap keeps the answer bounded
    std::fs::write(e.d.path().join("many.txt"), "needle\n".repeat(50)).unwrap();
    std::fs::write(e.d.path().join("many2.txt"), "needle\n".repeat(50)).unwrap();
    let m: Value = serde_json::from_str(&get("q=needle".into()).await.body).unwrap();
    assert!(m["matches"].as_array().unwrap().iter().filter(|x| x["path"] == "many.txt").count() <= 5);
}
