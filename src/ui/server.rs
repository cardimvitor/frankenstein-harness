use crate::config::{redact, Config, Env, StreamRedactor};
use crate::engine::{Engine, Io, NoticeKind, PlanDecision, TaskOptions, TaskResult};
use crate::session::checkpoint::Checkpoints;
use crate::skills::reuse::ReuseOffer;
use crate::skills::store::SkillStore;
use crate::types::Mode;
use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use rand::RngCore;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

/// Strict CSP: no inline script/style, no third-party origins, no framing, no form posts elsewhere.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; font-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

const INDEX: &str = include_str!("../../web/index.html");
const APP_JS: &str = include_str!("../../web/app.js");
const RENDER_JS: &str = include_str!("../../web/render.js");
const STYLE: &str = include_str!("../../web/style.css");

fn secret(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut b);
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn safe_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Clone, Debug)]
struct Ev {
    id: u64,
    kind: String,
    data: Value,
}

/// Event log + pending questions; shared by the engine's Io and the HTTP handlers.
pub struct Hub {
    events: Mutex<Vec<Ev>>,
    tx: broadcast::Sender<Ev>,
    asks: Mutex<HashMap<String, (String, oneshot::Sender<Value>)>>,
    next_id: Mutex<u64>,
    progress: Mutex<StreamRedactor>,
    reasoning: Mutex<StreamRedactor>,
    env: Env,
}

impl Hub {
    fn new(env: Env) -> Arc<Self> {
        let (tx, _) = broadcast::channel(4096);
        Arc::new(Hub { events: Mutex::new(vec![]), tx, asks: Mutex::new(HashMap::new()), next_id: Mutex::new(1), progress: Mutex::new(StreamRedactor::new(env.clone())), reasoning: Mutex::new(StreamRedactor::new(env.clone())), env })
    }

    fn push(&self, kind: &str, data: Value) {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            let id = *n;
            *n += 1;
            id
        };
        let ev = Ev { id, kind: kind.to_string(), data };
        {
            let mut e = self.events.lock().unwrap();
            e.push(ev.clone());
            if e.len() > 2000 {
                e.remove(0);
            }
        }
        let _ = self.tx.send(ev);
    }

    /// Emit an event; text held back for cross-chunk redaction is flushed first.
    fn emit(&self, kind: &str, data: Value) {
        let p = self.progress.lock().unwrap().flush();
        let r = self.reasoning.lock().unwrap().flush();
        if !p.is_empty() {
            self.push("progress", json!({"d": p}));
        }
        if !r.is_empty() {
            self.push("reasoning", json!({"d": r}));
        }
        self.push(kind, self.redact_value(data));
    }

    fn redact_value(&self, v: Value) -> Value {
        serde_json::from_str(&redact(&v.to_string(), &self.env)).unwrap_or(Value::Null)
    }

    async fn ask(&self, kind: &str, payload: Value, default: Value) -> Value {
        let id = secret(6);
        let (tx, rx) = oneshot::channel();
        self.asks.lock().unwrap().insert(id.clone(), (kind.to_string(), tx));
        self.emit("ask", json!({"id": id, "kind": kind, "payload": payload}));
        rx.await.unwrap_or(default)
    }

    fn answer(&self, id: &str, v: Value) -> bool {
        match self.asks.lock().unwrap().remove(id) {
            Some((_, tx)) => tx.send(v).is_ok(),
            None => false,
        }
    }

    fn cancel_asks(&self) {
        for (_, (kind, tx)) in self.asks.lock().unwrap().drain() {
            let _ = tx.send(match kind.as_str() {
                "questions" => json!([]),
                "plan" => json!({"ok": false}),
                "confirm" => json!(false),
                _ => json!([]),
            });
        }
    }
}

pub struct WebIo {
    hub: Arc<Hub>,
}

#[async_trait]
impl Io for WebIo {
    fn notice(&self, kind: NoticeKind, msg: &str) {
        self.hub.emit("notice", json!({"kind": kind.as_str(), "message": msg}));
    }
    fn progress(&self, d: &str) {
        let t = self.hub.progress.lock().unwrap().push(d);
        if !t.is_empty() {
            self.hub.push("progress", json!({"d": t}));
        }
    }
    fn reasoning(&self, d: &str) {
        let t = self.hub.reasoning.lock().unwrap().push(d);
        if !t.is_empty() {
            self.hub.push("reasoning", json!({"d": t}));
        }
    }
    fn tool_start(&self, name: &str, args: &Value) {
        self.hub.emit("tool_start", json!({"name": name, "args": args}));
    }
    fn tool_end(&self, name: &str, ok: bool, output: &str, ms: u64) {
        let out: String = output.chars().take(6000).collect();
        self.hub.emit("tool_end", json!({"name": name, "ok": ok, "output": out, "ms": ms}));
    }
    async fn ask_questions(&self, questions: Vec<String>) -> Vec<String> {
        let v = self.hub.ask("questions", json!({"questions": questions}), json!([])).await;
        v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
    }
    async fn approve_plan(&self, plan: &str, trivial: bool) -> PlanDecision {
        let v = self.hub.ask("plan", json!({"plan": plan, "trivial": trivial}), json!({"ok": false})).await;
        PlanDecision { ok: v["ok"] == true, feedback: v["feedback"].as_str().map(|s| s.to_string()) }
    }
    async fn confirm(&self, tool: &str, args: &Value) -> bool {
        self.hub.ask("confirm", json!({"tool": tool, "args": args}), json!(false)).await == json!(true)
    }
    async fn offer_reuse(&self, offers: &[ReuseOffer]) -> Vec<(String, Vec<String>)> {
        let payload = json!({"offers": offers.iter().map(|o| json!({"fromProject": o.from_project, "fromLabel": o.from_label, "skills": o.skills.iter().map(|s| json!({"id": s.id, "name": s.name, "summary": s.summary})).collect::<Vec<_>>()})).collect::<Vec<_>>()});
        let v = self.hub.ask("reuse", payload, json!([])).await;
        v.as_array().map(|a| a.iter().filter_map(|p| Some((p["fromProject"].as_str()?.to_string(), p["ids"].as_array()?.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()))).collect()).unwrap_or_default()
    }
}

struct App {
    cfg: Config,
    hub: Arc<Hub>,
    engine: Arc<Engine>,
    sessions: Mutex<HashSet<String>>,
    code: Mutex<String>,
    failures: Mutex<u32>,
    busy: Mutex<Option<CancellationToken>>,
    last: Mutex<Option<String>>,
    allowed_hosts: HashSet<String>,
    project_id: String,
    log: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

pub struct WebServer {
    pub url: String,
    pub port: u16,
    app: Arc<App>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl WebServer {
    pub fn code(&self) -> String {
        self.app.code.lock().unwrap().clone()
    }
    pub fn stop(&mut self) {
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn hardened(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert("content-security-policy", HeaderValue::from_static(CSP));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    r
}

fn err(status: StatusCode, msg: &str) -> Response {
    hardened((status, Json(json!({"error": msg}))).into_response())
}

fn cookie_of(h: &HeaderMap) -> Option<String> {
    let c = h.get(header::COOKIE)?.to_str().ok()?;
    c.split(';').map(|p| p.trim()).find_map(|p| p.strip_prefix("fh_session=").map(|v| v.to_string()))
}

fn authed(app: &App, h: &HeaderMap) -> bool {
    match cookie_of(h) {
        Some(c) => app.sessions.lock().unwrap().iter().any(|s| safe_eq(s, &c)),
        None => false,
    }
}

/// Host/Origin/CSRF guard applied to every request; hardening headers on every response.
async fn guard(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    if !app.allowed_hosts.contains(&host) {
        return err(StatusCode::FORBIDDEN, "bad host");
    }
    if let Some(origin) = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if origin != format!("http://{host}") {
            return err(StatusCode::FORBIDDEN, "bad origin");
        }
    }
    let is_api = req.uri().path().starts_with("/api/");
    if is_api && req.method() == Method::POST {
        let ct = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
        if req.headers().get("x-fh").and_then(|v| v.to_str().ok()) != Some("1") || !ct.starts_with("application/json") {
            return err(StatusCode::FORBIDDEN, "bad request headers");
        }
    }
    if is_api && req.method() == Method::OPTIONS {
        return err(StatusCode::FORBIDDEN, "cors not supported");
    }
    hardened(next.run(req).await)
}

fn asset(body: &'static str, ct: &'static str) -> Response {
    ([(header::CONTENT_TYPE, ct)], body).into_response()
}

async fn auth(State(app): State<Arc<App>>, Json(b): Json<Value>) -> Response {
    let code = app.code.lock().unwrap().clone();
    if b["code"].as_str().map(|c| safe_eq(c, &code)).unwrap_or(false) {
        let token = secret(32);
        app.sessions.lock().unwrap().insert(token.clone());
        *app.code.lock().unwrap() = secret(9); // one-time: consumed
        *app.failures.lock().unwrap() = 0;
        let mut r = Json(json!({"ok": true})).into_response();
        r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&format!("fh_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=86400")).unwrap());
        return r;
    }
    let mut f = app.failures.lock().unwrap();
    *f += 1;
    if *f >= 5 {
        let c = secret(9);
        *app.code.lock().unwrap() = c.clone();
        *f = 0;
        if let Some(l) = &app.log {
            l(&format!("too many failed logins; new code: {c}"));
        }
    }
    err(StatusCode::UNAUTHORIZED, "invalid code")
}

async fn events(State(app): State<Arc<App>>, headers: HeaderMap, axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    let last: u64 = headers.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok()).or_else(|| q.get("after").and_then(|s| s.parse().ok())).unwrap_or(0);
    // subscribe before replaying so no event is lost in between
    let rx = app.hub.tx.subscribe();
    let backlog: Vec<Ev> = app.hub.events.lock().unwrap().iter().filter(|e| e.id > last).cloned().collect();
    let max_replayed = backlog.last().map(|e| e.id).unwrap_or(last);
    let to_event = |e: Ev| Ok::<Event, Infallible>(Event::default().id(e.id.to_string()).event(e.kind).data(e.data.to_string()));
    let replay = futures_util::stream::iter(backlog.into_iter().map(to_event));
    let live = BroadcastStream::new(rx).filter_map(move |r| async move { r.ok().filter(|e| e.id > max_replayed) }).map(to_event);
    hardened(Sse::new(replay.chain(live)).keep_alive(KeepAlive::default()).into_response())
}

async fn state(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    let last = app.last.lock().unwrap().clone();
    Json(json!({"busy": app.busy.lock().unwrap().is_some(), "cwd": app.engine.cwd.to_string_lossy(), "model": app.cfg.model, "last": last.map(|v| json!({"verdict": v}))})).into_response()
}

async fn activity(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    Json(app.engine.store.activity(50).into_iter().map(|a| json!({"ts": a.ts, "kind": a.kind, "skill": a.skill, "reason": a.reason})).collect::<Vec<_>>()).into_response()
}

async fn tree(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    let files = crate::tools::fs::list_all(&app.engine.cwd, 3000);
    Json(json!({"files": files})).into_response()
}

async fn history(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    Json(app.engine.store.history("").into_iter().map(|(name, v, hash, reason, diff)| json!({"skill": name, "version": v, "hash": hash, "reason": reason, "diff": diff.chars().take(2000).collect::<String>()})).collect::<Vec<_>>()).into_response()
}

const FILE_LIMIT: usize = 200_000;

/// Files that are never served through the browser even inside the workspace.
fn withheld(rel: &str) -> bool {
    let l = rel.to_lowercase();
    let name = l.rsplit('/').next().unwrap_or("");
    l == ".git" || l.starts_with(".git/") || name == ".env" || name.starts_with(".env.") || name.ends_with(".pem") || name.ends_with(".key") || name.starts_with("id_rsa") || name.starts_with("id_ed25519") || name == ".netrc" || name == ".npmrc"
}

async fn file(State(app): State<Arc<App>>, headers: HeaderMap, axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    let Some(p) = q.get("path").filter(|p| !p.is_empty() && p.len() < 1024) else { return err(StatusCode::BAD_REQUEST, "path required") };
    let cwd = &app.engine.cwd;
    let Ok(abs) = crate::util::paths::in_workspace(cwd, p) else { return err(StatusCode::FORBIDDEN, "path escapes the workspace") };
    let rel = crate::util::paths::rel(cwd, &abs);
    if withheld(&rel) {
        return err(StatusCode::FORBIDDEN, "this file is not served through the browser");
    }
    let Ok(bytes) = std::fs::read(&abs) else { return err(StatusCode::NOT_FOUND, "no such file") };
    let head = &bytes[..bytes.len().min(8000)];
    if head.contains(&0) {
        return Json(json!({"path": rel, "binary": true, "size": bytes.len()})).into_response();
    }
    let truncated = bytes.len() > FILE_LIMIT;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(FILE_LIMIT)]).into_owned();
    Json(json!({"path": rel, "binary": false, "size": bytes.len(), "truncated": truncated, "content": redact(&text, &app.engine.env)})).into_response()
}

const MAX_MATCHES: usize = 200;
const PER_FILE: usize = 5;

/// Case-insensitive literal search in the workspace: matching file names first, then matching lines.
/// `kind=sessions` searches past task sessions of this repository instead.
async fn search(State(app): State<Arc<App>>, headers: HeaderMap, axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    let query = q.get("q").map(|s| s.trim().to_string()).unwrap_or_default();
    if query.is_empty() || query.len() > 200 {
        return err(StatusCode::BAD_REQUEST, "q required (at most 200 characters)");
    }
    if q.get("kind").map(|k| k == "sessions").unwrap_or(false) {
        let hits = crate::session::search::search(&app.engine.env, Some(&app.project_id), &query, 20);
        return Json(json!({"sessions": hits.into_iter().map(|h| json!({"id": h.id, "started": h.started, "verdict": h.verdict, "task": h.task, "snippet": h.snippet})).collect::<Vec<_>>()})).into_response();
    }
    let (cwd, env) = (app.engine.cwd.clone(), app.engine.env.clone());
    let out = tokio::task::spawn_blocking(move || {
        let needle = query.to_lowercase();
        let files = crate::tools::fs::list_all(&cwd, 3000);
        let mut names: Vec<&String> = files.iter().filter(|f| f.to_lowercase().contains(&needle)).collect();
        // a match in the file's own name ranks above a match in a directory name
        names.sort_by_key(|f| (!f.rsplit('/').next().unwrap_or("").to_lowercase().contains(&needle), f.len()));
        let names: Vec<String> = names.into_iter().take(30).cloned().collect();
        let mut matches = Vec::new();
        let mut truncated = false;
        'files: for f in &files {
            if withheld(f) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(cwd.join(f)) else { continue };
            if meta.len() as usize > FILE_LIMIT {
                continue;
            }
            let Ok(bytes) = std::fs::read(cwd.join(f)) else { continue };
            if bytes[..bytes.len().min(8000)].contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let mut in_file = 0;
            for (i, line) in text.lines().enumerate() {
                if line.to_lowercase().contains(&needle) {
                    matches.push(json!({"path": f, "line": i + 1, "text": redact(&line.trim().chars().take(160).collect::<String>(), &env)}));
                    in_file += 1;
                    if matches.len() >= MAX_MATCHES {
                        truncated = true;
                        break 'files;
                    }
                    if in_file >= PER_FILE {
                        break;
                    }
                }
            }
        }
        json!({"names": names, "matches": matches, "truncated": truncated})
    })
    .await
    .unwrap_or_else(|_| json!({"names": [], "matches": [], "truncated": false}));
    Json(out).into_response()
}

async fn sessions(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    let rows = crate::session::log::list(&app.engine.env, &app.project_id);
    Json(rows.into_iter().take(30).map(|s| json!({"id": s.id, "task": redact(&s.task.chars().take(200).collect::<String>(), &app.engine.env), "verdict": s.verdict, "reason": s.reason, "started": s.started, "resumable": s.verdict.is_none() && s.plan.is_some() && s.checkpoint.is_some()})).collect::<Vec<_>>()).into_response()
}

async fn resume(State(app): State<Arc<App>>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    let (text, state) = match crate::engine::load_resume(&app.engine.env, &app.engine.cwd, b["id"].as_str()) {
        Ok(x) => x,
        Err(e) => return err(StatusCode::BAD_REQUEST, &e),
    };
    let token = {
        let mut busy = app.busy.lock().unwrap();
        if busy.is_some() {
            return err(StatusCode::CONFLICT, "a task is already running");
        }
        let t = CancellationToken::new();
        *busy = Some(t.clone());
        t
    };
    let auto = b["mode"] == "auto";
    let approval = b["approval"].as_str().and_then(Mode::parse).unwrap_or(Mode::AutoEdit);
    app.hub.emit("busy", json!({"busy": true, "task": format!("resuming: {}", text.chars().take(160).collect::<String>())}));
    let a2 = app.clone();
    tokio::spawn(async move {
        let r: TaskResult = a2.engine.run_task(&text, TaskOptions { auto, approval, cancel: Some(token), resume: Some(state), ..Default::default() }).await;
        *a2.last.lock().unwrap() = Some(r.verdict.clone());
        a2.hub.emit("result", r.to_json());
        *a2.busy.lock().unwrap() = None;
        a2.hub.emit("busy", json!({"busy": false}));
        a2.engine.drain(20_000).await;
    });
    (StatusCode::ACCEPTED, Json(json!({"ok": true}))).into_response()
}

async fn task(State(app): State<Arc<App>>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    let Some(text) = b["task"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.len() <= 20000) else { return err(StatusCode::BAD_REQUEST, "task required") };
    let token = {
        let mut busy = app.busy.lock().unwrap();
        if busy.is_some() {
            return err(StatusCode::CONFLICT, "a task is already running");
        }
        let t = CancellationToken::new();
        *busy = Some(t.clone());
        t
    };
    let auto = b["mode"] == "auto";
    let approval = b["approval"].as_str().and_then(Mode::parse).unwrap_or(Mode::AutoEdit);
    app.hub.emit("busy", json!({"busy": true, "task": text.chars().take(200).collect::<String>()}));
    let a2 = app.clone();
    let sandbox = b["sandbox"] == true;
    tokio::spawn(async move {
        let r: TaskResult = a2.engine.run_task(&text, TaskOptions { auto, approval, sandbox, cancel: Some(token), ..Default::default() }).await;
        *a2.last.lock().unwrap() = Some(r.verdict.clone());
        a2.hub.emit("result", r.to_json());
        *a2.busy.lock().unwrap() = None;
        a2.hub.emit("busy", json!({"busy": false}));
        a2.engine.drain(20_000).await;
    });
    (StatusCode::ACCEPTED, Json(json!({"ok": true}))).into_response()
}

async fn answer(State(app): State<Arc<App>>, headers: HeaderMap, Json(b): Json<Value>) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    if app.hub.answer(b["id"].as_str().unwrap_or(""), b["value"].clone()) {
        Json(json!({"ok": true})).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "no such question")
    }
}

async fn cancel(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    if let Some(t) = app.busy.lock().unwrap().as_ref() {
        t.cancel();
    }
    app.hub.cancel_asks();
    Json(json!({"ok": true})).into_response()
}

async fn undo(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !authed(&app, &headers) {
        return err(StatusCode::UNAUTHORIZED, "not authenticated");
    }
    let is_repo = Checkpoints::new(app.engine.cwd.clone()).is_repo().await;
    Json(json!({"ok": true, "note": if is_repo { "use the rolled-back patch in .fh/rejected or git; per-turn undo runs through the CLI (fh undo)" } else { "not a git repository" }})).into_response()
}

pub struct ServeOptions {
    pub port: u16,
    pub store: Option<SkillStore>,
    pub log: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

pub async fn start_web_server(cfg: Config, env: Env, cwd: PathBuf, o: ServeOptions) -> anyhow::Result<WebServer> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", o.port)).await?;
    let port = listener.local_addr()?.port();
    let hub = Hub::new(env.clone());
    let store = match o.store {
        Some(s) => s,
        None => {
            let (s, note) = SkillStore::open_resilient(&env);
            if let (Some(n), Some(l)) = (note, &o.log) {
                l(&format!("warning: {n}"));
            }
            s
        }
    };
    let app_cwd = cwd.clone();
    let engine = Arc::new(Engine::new(cfg.clone(), env, Arc::new(WebIo { hub: hub.clone() }), cwd, store));
    let app = Arc::new(App {
        cfg,
        hub,
        engine,
        sessions: Mutex::new(HashSet::new()),
        code: Mutex::new(secret(9)),
        failures: Mutex::new(0),
        busy: Mutex::new(None),
        last: Mutex::new(None),
        project_id: crate::fingerprint::fingerprint(&app_cwd).project_id,
        allowed_hosts: [format!("127.0.0.1:{port}"), format!("localhost:{port}")].into_iter().collect(),
        log: o.log,
    });
    let router = Router::new()
        .route("/", get(|| async { asset(INDEX, "text/html; charset=utf-8") }))
        .route("/app.js", get(|| async { asset(APP_JS, "text/javascript; charset=utf-8") }))
        .route("/render.js", get(|| async { asset(RENDER_JS, "text/javascript; charset=utf-8") }))
        .route("/style.css", get(|| async { asset(STYLE, "text/css; charset=utf-8") }))
        .route("/api/auth", post(auth))
        .route("/api/events", get(events))
        .route("/api/state", get(state))
        .route("/api/activity", get(activity))
        .route("/api/tree", get(tree))
        .route("/api/history", get(history))
        .route("/api/file", get(file))
        .route("/api/search", get(search))
        .route("/api/sessions", get(sessions))
        .route("/api/resume", post(resume))
        .route("/api/task", post(task))
        .route("/api/answer", post(answer))
        .route("/api/cancel", post(cancel))
        .route("/api/undo", post(undo))
        .fallback(|| async { err(StatusCode::NOT_FOUND, "not found") })
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .layer(axum::extract::DefaultBodyLimit::max(1_000_000))
        .with_state(app.clone());
    let (tx, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).with_graceful_shutdown(async move { let _ = rx.await; }).await;
    });
    let _ = Body::empty();
    Ok(WebServer { url: format!("http://127.0.0.1:{port}/"), port, app, shutdown: Some(tx) })
}
