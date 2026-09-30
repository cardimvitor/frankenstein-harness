//! LSP diagnostics as a verifier input. A language server (TypeScript, Python, C#, Go, or one the user
//! configures) is started, each changed file is opened with its base-checkpoint content and then with its
//! new content, and only diagnostics that are NEW relative to the base count. Build and test output
//! remain the arbiters when no server is installed. Project `.fh/lsp.json` (it runs commands) needs a trusted workspace.
use super::checks::{CheckKind, CheckResult, Status};
use crate::config::{config_dir, Env};
use crate::session::checkpoint::Checkpoints;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct LspSpec {
    pub name: String,
    pub cmd: String,
    pub args: Vec<String>,
    pub exts: Vec<String>,
    /// languageId per extension; defaults to the extension
    pub language_ids: HashMap<String, String>,
    pub timeout_ms: u64,
}

fn on_path(bin: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    std::process::Command::new(finder).arg(bin).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn spec(name: &str, cmd: &str, args: &[&str], exts: &[&str], ids: &[(&str, &str)]) -> LspSpec {
    LspSpec { name: name.into(), cmd: cmd.into(), args: args.iter().map(|s| s.to_string()).collect(), exts: exts.iter().map(|s| s.to_string()).collect(), language_ids: ids.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), timeout_ms: 25_000 }
}

fn has_file_ext(cwd: &Path, ext: &str) -> bool {
    crate::tools::fs::list_all(cwd, 3000).iter().any(|f| f.ends_with(ext))
}

/// Built-in servers that apply to this repository, plus user/project `lsp.json` entries.
pub fn load_specs(cwd: &Path, env: &Env) -> Vec<LspSpec> {
    let mut out = Vec::new();
    let local_bin = |n: &str| -> Option<String> {
        let p = cwd.join("node_modules/.bin").join(n);
        p.exists().then(|| p.to_string_lossy().to_string())
    };
    if cwd.join("tsconfig.json").exists() {
        let ts_major: Option<u32> = std::fs::read_to_string(cwd.join("node_modules/typescript/package.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v["version"].as_str().and_then(|v| v.split('.').next().and_then(|m| m.parse().ok())));
        let ids = [("ts", "typescript"), ("tsx", "typescriptreact"), ("js", "javascript"), ("jsx", "javascriptreact")];
        let exts = ["ts", "tsx", "js", "jsx"];
        match (ts_major, local_bin("tsc")) {
            // TypeScript 7+ ships a native language server (`tsc --lsp`); typescript-language-server needs the JS tsserver it no longer has
            (Some(m), Some(tsc)) if m >= 7 => out.push(spec("typescript", &tsc, &["--lsp", "-stdio"], &exts, &ids)),
            _ => {
                if let Some(c) = local_bin("typescript-language-server").or_else(|| on_path("typescript-language-server").then(|| "typescript-language-server".to_string())) {
                    out.push(spec("typescript", &c, &["--stdio"], &exts, &ids));
                }
            }
        }
    }
    if has_file_ext(cwd, ".py") && on_path("pyright-langserver") {
        out.push(spec("python", "pyright-langserver", &["--stdio"], &["py"], &[("py", "python")]));
    }
    if (has_file_ext(cwd, ".sln") || has_file_ext(cwd, ".csproj")) && on_path("csharp-ls") {
        out.push(spec("csharp", "csharp-ls", &[], &["cs"], &[("cs", "csharp")]));
    }
    if cwd.join("go.mod").exists() && on_path("gopls") {
        out.push(spec("go", "gopls", &[], &["go"], &[("go", "go")]));
    }
    let mut files: Vec<PathBuf> = vec![config_dir(env).join("lsp.json")];
    if crate::trust::is_trusted(env, cwd) {
        files.push(cwd.join(".fh").join("lsp.json"));
    }
    for f in files {
        let Some(v) = std::fs::read_to_string(&f).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { continue };
        for e in v.as_array().cloned().unwrap_or_default() {
            let (Some(cmd), Some(exts)) = (e["cmd"].as_str(), e["exts"].as_array()) else { continue };
            out.push(LspSpec {
                name: e["name"].as_str().unwrap_or(cmd).to_string(),
                cmd: cmd.to_string(),
                args: e["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default(),
                exts: exts.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
                language_ids: e["languageIds"].as_object().map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect()).unwrap_or_default(),
                timeout_ms: e["timeoutMs"].as_u64().unwrap_or(25_000),
            });
        }
    }
    out
}

struct Client {
    stdin: ChildStdin,
    rx: mpsc::UnboundedReceiver<Value>,
    _child: Child,
    next: u64,
    pull: bool,
}

fn uri(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    format!("file://{}{}", if s.starts_with('/') { "" } else { "/" }, s.replace(' ', "%20"))
}

async fn send(stdin: &mut ChildStdin, v: &Value) -> std::io::Result<()> {
    let body = v.to_string();
    stdin.write_all(format!("Content-Length: {}\r\n\r\n{}", body.len(), body).as_bytes()).await?;
    stdin.flush().await
}

impl Client {
    async fn start(s: &LspSpec, root: &Path) -> Result<Client, String> {
        let mut c = Command::new(&s.cmd);
        c.args(&s.args).current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        let mut child = c.spawn().map_err(|e| format!("cannot start {}: {e}", s.cmd))?;
        let stdin = child.stdin.take().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    match out.read_line(&mut line).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    let l = line.trim();
                    if l.is_empty() {
                        break;
                    }
                    if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut buf = vec![0u8; len];
                if out.read_exact(&mut buf).await.is_err() {
                    return;
                }
                if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                    if tx.send(v).is_err() {
                        return;
                    }
                }
            }
        });
        let mut cl = Client { stdin, rx, _child: child, next: 1, pull: false };
        let id = cl.next;
        cl.next += 1;
        let root_uri = uri(root);
        send(&mut cl.stdin, &json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
            "processId": std::process::id(), "rootUri": root_uri, "workspaceFolders": [{"uri": root_uri, "name": "root"}],
            "capabilities": {"textDocument": {"publishDiagnostics": {}, "synchronization": {}}, "workspace": {"configuration": true, "workspaceFolders": true}}}}))
        .await
        .map_err(|e| e.to_string())?;
        let init = cl.wait_response(id, Duration::from_millis(s.timeout_ms)).await.ok_or_else(|| format!("{} did not answer initialize", s.name))?;
        cl.pull = init.pointer("/result/capabilities/diagnosticProvider").map(|d| !d.is_null()).unwrap_or(false);
        send(&mut cl.stdin, &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})).await.map_err(|e| e.to_string())?;
        // some servers (pyright) only start analysing after the first configuration exchange
        send(&mut cl.stdin, &json!({"jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": {"settings": {}}})).await.map_err(|e| e.to_string())?;
        Ok(cl)
    }

    /// Answers server-initiated requests so the server never blocks on us.
    async fn auto_reply(&mut self, m: &Value) {
        if let (Some(id), Some(method)) = (m.get("id"), m.get("method").and_then(|x| x.as_str())) {
            let result = if method == "workspace/configuration" { Value::Array(m["params"]["items"].as_array().map(|a| a.iter().map(|_| Value::Null).collect()).unwrap_or_default()) } else { Value::Null };
            let _ = send(&mut self.stdin, &json!({"jsonrpc": "2.0", "id": id, "result": result})).await;
        }
    }

    async fn wait_response(&mut self, id: u64, timeout: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let m = tokio::time::timeout_at(deadline, self.rx.recv()).await.ok()??;
            if m.get("method").is_some() {
                self.auto_reply(&m).await;
                continue;
            }
            if m.get("id").and_then(|i| i.as_u64()) == Some(id) {
                return Some(m);
            }
        }
    }

    /// Diagnostics for `file_uri`: pull if supported, else the latest push after a quiet period.
    async fn diagnostics(&mut self, file_uri: &str, timeout: Duration) -> Vec<Value> {
        if self.pull {
            let id = self.next;
            self.next += 1;
            let _ = send(&mut self.stdin, &json!({"jsonrpc": "2.0", "id": id, "method": "textDocument/diagnostic", "params": {"textDocument": {"uri": file_uri}}})).await;
            if let Some(r) = self.wait_response(id, timeout).await {
                return r.pointer("/result/items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
            }
            return vec![];
        }
        let deadline = tokio::time::Instant::now() + timeout;
        let mut latest: Option<Vec<Value>> = None;
        loop {
            let wait = if latest.is_some() { Duration::from_millis(600) } else { deadline.saturating_duration_since(tokio::time::Instant::now()) };
            let Ok(Some(m)) = tokio::time::timeout(wait, self.rx.recv()).await else { break };
            if m.get("method").is_some() {
                if m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == file_uri {
                    latest = Some(m["params"]["diagnostics"].as_array().cloned().unwrap_or_default());
                } else {
                    self.auto_reply(&m).await;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
        }
        latest.unwrap_or_default()
    }

    async fn open(&mut self, file_uri: &str, lang: &str, version: u32, text: &str) {
        let _ = send(&mut self.stdin, &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": file_uri, "languageId": lang, "version": version, "text": text}}})).await;
    }

    async fn change(&mut self, file_uri: &str, version: u32, text: &str) {
        let _ = send(&mut self.stdin, &json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {"textDocument": {"uri": file_uri, "version": version}, "contentChanges": [{"text": text}]}})).await;
    }

    async fn shutdown(mut self) {
        let id = self.next;
        let _ = send(&mut self.stdin, &json!({"jsonrpc": "2.0", "id": id, "method": "shutdown"})).await;
        let _ = self.wait_response(id, Duration::from_secs(2)).await;
        let _ = send(&mut self.stdin, &json!({"jsonrpc": "2.0", "method": "exit"})).await;
    }
}

fn is_error(d: &Value) -> bool {
    d["severity"].as_u64().unwrap_or(1) == 1
}

/// Identity of a diagnostic that survives line shifts.
fn key(d: &Value) -> String {
    format!("{}|{}", d["code"].to_string().trim_matches('"'), d["message"].as_str().unwrap_or(""))
}

fn new_errors(base: &[Value], cur: &[Value]) -> Vec<Value> {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for d in base.iter().filter(|d| is_error(d)) {
        *counts.entry(key(d)).or_default() += 1;
    }
    let mut out = Vec::new();
    for d in cur.iter().filter(|d| is_error(d)) {
        let c = counts.entry(key(d)).or_default();
        if *c > 0 {
            *c -= 1;
        } else {
            out.push(d.clone());
        }
    }
    out
}

/// None when no language server applies to the changed files.
pub async fn diagnostics_check(cwd: &Path, cp: &Checkpoints, base: &str, changed: &[String], specs: &[LspSpec]) -> Option<CheckResult> {
    let t0 = std::time::Instant::now();
    let mut problems: Vec<String> = Vec::new();
    let mut ran: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for s in specs {
        let files: Vec<&String> = changed.iter().filter(|f| cwd.join(f).is_file() && Path::new(f.as_str()).extension().and_then(|e| e.to_str()).map(|e| s.exts.iter().any(|x| x == e)).unwrap_or(false)).collect();
        if files.is_empty() {
            continue;
        }
        let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let mut cl = match Client::start(s, &root).await {
            Ok(c) => c,
            Err(e) => {
                skipped.push(e);
                continue;
            }
        };
        ran.push(s.name.clone());
        let timeout = Duration::from_millis(s.timeout_ms);
        for f in files {
            let ext = Path::new(f.as_str()).extension().and_then(|e| e.to_str()).unwrap_or("");
            let lang = s.language_ids.get(ext).cloned().unwrap_or_else(|| ext.to_string());
            let cur_text = std::fs::read_to_string(cwd.join(f)).unwrap_or_default();
            let file_uri = uri(&root.join(f));
            let base_text = cp.file_at(base, f).await;
            let base_diags = match &base_text {
                Some(bt) => {
                    cl.open(&file_uri, &lang, 1, bt).await;
                    let d = cl.diagnostics(&file_uri, timeout).await;
                    cl.change(&file_uri, 2, &cur_text).await;
                    d
                }
                None => {
                    cl.open(&file_uri, &lang, 1, &cur_text).await;
                    vec![]
                }
            };
            let cur = cl.diagnostics(&file_uri, timeout).await;
            for d in new_errors(&base_diags, &cur) {
                let line = d.pointer("/range/start/line").and_then(|l| l.as_u64()).unwrap_or(0) + 1;
                problems.push(format!("{f}:{line}: {} ({})", d["message"].as_str().unwrap_or("").lines().next().unwrap_or(""), s.name));
            }
        }
        cl.shutdown().await;
    }
    if ran.is_empty() {
        return if skipped.is_empty() { None } else { Some(CheckResult { name: "diagnostics".into(), kind: CheckKind::Types, status: Status::Skipped, detail: skipped.join("; "), ms: t0.elapsed().as_millis() as u64 }) };
    }
    problems.truncate(40);
    Some(CheckResult {
        name: "diagnostics".into(),
        kind: CheckKind::Types,
        status: if problems.is_empty() { Status::Pass } else { Status::Fail },
        detail: if problems.is_empty() { format!("no new errors ({})", ran.join(", ")) } else { format!("new language-server errors introduced by this change:\n{}", problems.join("\n")) },
        ms: t0.elapsed().as_millis() as u64,
    })
}
