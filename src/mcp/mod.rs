//! MCP client (Model Context Protocol): stdio and streamable-HTTP transports, tool discovery and calls.
//! Config: `.fh/mcp.json` (project, trusted workspaces only) and `<config dir>/mcp.json` (user), both in the
//! `{"mcpServers": {name: {command,args,env} | {url,headers}}}` shape used by other MCP clients.
//! Secrets: `${VAR}` reads the environment, `${keychain:ACCOUNT}` the OS keychain; nothing secret lives in the file.
//! Tools appear as `mcp__<server>__<tool>`, are treated as mutating unless the server sets `readOnlyHint`.
use crate::config::{config_dir, Env};
use crate::tools::{Tool, ToolCtx, ToolRef, ToolResult};
use crate::types::ToolSpec;
use crate::util::proc::{clip, scrubbed_env};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

const PROTOCOL: &str = "2025-03-26";

#[derive(Clone, Debug)]
pub enum ServerConfig {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String>, cwd: Option<String> },
    Http { url: String, headers: HashMap<String, String> },
}

/// `${VAR}` from the environment, `${keychain:ACCOUNT}` from the OS keychain. Unknown references stay empty.
pub fn expand(s: &str, env: &Env) -> String {
    let re = regex::Regex::new(r"\$\{([^}]+)\}").unwrap();
    re.replace_all(s, |c: &regex::Captures| {
        let k = &c[1];
        if let Some(acct) = k.strip_prefix("keychain:") {
            crate::secrets::get(acct).unwrap_or_default()
        } else {
            env.get(k).cloned().unwrap_or_default()
        }
    })
    .into_owned()
}

pub fn parse_config(v: &Value, env: &Env) -> Vec<(String, ServerConfig)> {
    let mut out = Vec::new();
    let Some(m) = v.get("mcpServers").and_then(|x| x.as_object()) else { return out };
    for (name, c) in m {
        let strs = |k: &str| -> HashMap<String, String> { c.get(k).and_then(|x| x.as_object()).map(|o| o.iter().filter_map(|(a, b)| b.as_str().map(|b| (a.clone(), expand(b, env)))).collect()).unwrap_or_default() };
        if let Some(url) = c.get("url").and_then(|u| u.as_str()) {
            out.push((name.clone(), ServerConfig::Http { url: expand(url, env), headers: strs("headers") }));
        } else if let Some(cmd) = c.get("command").and_then(|u| u.as_str()) {
            let args = c.get("args").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| expand(s, env))).collect()).unwrap_or_default();
            out.push((name.clone(), ServerConfig::Stdio { command: expand(cmd, env), args, env: strs("env"), cwd: c.get("cwd").and_then(|x| x.as_str()).map(|s| s.to_string()) }));
        }
    }
    out
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

enum Transport {
    Stdio { stdin: tokio::sync::Mutex<ChildStdin>, pending: Pending, _child: Mutex<Child> },
    Http { client: reqwest::Client, url: String, headers: HashMap<String, String>, session: Mutex<Option<String>> },
}

pub struct McpClient {
    pub name: String,
    t: Transport,
    next: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub schema: Value,
    pub read_only: bool,
}

impl McpClient {
    pub async fn connect(name: &str, cfg: &ServerConfig, cwd: &Path) -> Result<Arc<McpClient>, String> {
        let t = match cfg {
            ServerConfig::Stdio { command, args, env, cwd: c } => {
                let mut cmd = Command::new(command);
                cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
                cmd.current_dir(c.as_deref().map(|p| cwd.join(p)).unwrap_or_else(|| cwd.to_path_buf()));
                cmd.env_clear();
                for (k, v) in scrubbed_env().into_iter().chain(env.clone()) {
                    cmd.env(k, v);
                }
                let mut child = cmd.spawn().map_err(|e| format!("cannot start MCP server \"{name}\" ({command}): {e}"))?;
                let stdin = child.stdin.take().unwrap();
                let stdout = child.stdout.take().unwrap();
                let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
                let p2 = pending.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stdout).lines();
                    while let Ok(Some(l)) = lines.next_line().await {
                        let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
                        if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                            if v.get("method").is_none() {
                                if let Some(tx) = p2.lock().unwrap().remove(&id) {
                                    let _ = tx.send(v);
                                }
                            }
                        }
                    }
                    // server exited: fail everything still waiting
                    p2.lock().unwrap().clear();
                });
                Transport::Stdio { stdin: tokio::sync::Mutex::new(stdin), pending, _child: Mutex::new(child) }
            }
            ServerConfig::Http { url, headers } => Transport::Http { client: reqwest::Client::new(), url: url.clone(), headers: headers.clone(), session: Mutex::new(None) },
        };
        let c = Arc::new(McpClient { name: name.to_string(), t, next: AtomicU64::new(1) });
        c.request("initialize", json!({"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "frankenstein-harness", "version": env!("CARGO_PKG_VERSION")}}), Duration::from_secs(30), None).await?;
        c.notify("notifications/initialized", json!({})).await;
        Ok(c)
    }

    async fn write_line(&self, v: &Value) -> Result<(), String> {
        if let Transport::Stdio { stdin, .. } = &self.t {
            let mut s = stdin.lock().await;
            s.write_all(format!("{v}\n").as_bytes()).await.map_err(|e| format!("write to MCP server failed: {e}"))?;
            s.flush().await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn notify(&self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        match &self.t {
            Transport::Stdio { .. } => {
                let _ = self.write_line(&msg).await;
            }
            Transport::Http { .. } => {
                let _ = self.http_post(&msg).await;
            }
        }
    }

    async fn http_post(&self, body: &Value) -> Result<(String, String), String> {
        let Transport::Http { client, url, headers, session } = &self.t else { return Err("not http".into()) };
        let mut rb = client.post(url).header("content-type", "application/json").header("accept", "application/json, text/event-stream").header("mcp-protocol-version", PROTOCOL);
        for (k, v) in headers {
            rb = rb.header(k, v);
        }
        if let Some(s) = session.lock().unwrap().clone() {
            rb = rb.header("mcp-session-id", s);
        }
        let resp = rb.body(body.to_string()).send().await.map_err(|e| format!("MCP http error: {e}"))?;
        if let Some(s) = resp.headers().get("mcp-session-id").and_then(|h| h.to_str().ok()) {
            *session.lock().unwrap() = Some(s.to_string());
        }
        let status = resp.status();
        let ct = resp.headers().get("content-type").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("MCP http {status}: {}", clip(&text, 300)));
        }
        Ok((ct, text))
    }

    /// JSON-RPC request; returns `result`, or the error message.
    pub async fn request(&self, method: &str, params: Value, timeout: Duration, cancel: Option<&tokio_util::sync::CancellationToken>) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let never = tokio_util::sync::CancellationToken::new();
        let cancel = cancel.unwrap_or(&never);
        let fut = async {
            match &self.t {
                Transport::Stdio { pending, .. } => {
                    let (tx, rx) = oneshot::channel();
                    pending.lock().unwrap().insert(id, tx);
                    self.write_line(&msg).await?;
                    rx.await.map_err(|_| "MCP server closed the connection".to_string())
                }
                Transport::Http { .. } => {
                    let (ct, text) = self.http_post(&msg).await?;
                    if ct.contains("text/event-stream") {
                        for l in text.lines() {
                            if let Some(d) = l.strip_prefix("data:") {
                                if let Ok(v) = serde_json::from_str::<Value>(d.trim()) {
                                    if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                                        return Ok(v);
                                    }
                                }
                            }
                        }
                        Err("MCP http stream ended without a response".to_string())
                    } else {
                        serde_json::from_str::<Value>(&text).map_err(|e| format!("bad MCP response: {e}"))
                    }
                }
            }
        };
        let res = tokio::select! {
            r = tokio::time::timeout(timeout, fut) => match r { Ok(r) => r, Err(_) => Err(format!("MCP request \"{method}\" timed out after {} s", timeout.as_secs())) },
            _ = cancel.cancelled() => Err("cancelled".to_string()),
        };
        if let Transport::Stdio { pending, .. } = &self.t {
            pending.lock().unwrap().remove(&id);
        }
        if matches!(&res, Err(e) if e == "cancelled" || e.contains("timed out")) {
            self.notify("notifications/cancelled", json!({"requestId": id, "reason": "client gave up"})).await;
        }
        let v = res?;
        if let Some(e) = v.get("error") {
            return Err(e.get("message").and_then(|m| m.as_str()).unwrap_or("MCP error").to_string());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>, String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let params = cursor.as_ref().map(|c| json!({"cursor": c})).unwrap_or_else(|| json!({}));
            let r = self.request("tools/list", params, Duration::from_secs(30), None).await?;
            for t in r.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
                let Some(name) = t.get("name").and_then(|n| n.as_str()) else { continue };
                out.push(McpToolInfo {
                    name: name.to_string(),
                    description: t.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
                    schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    read_only: t.pointer("/annotations/readOnlyHint").and_then(|b| b.as_bool()).unwrap_or(false),
                });
            }
            cursor = r.get("nextCursor").and_then(|c| c.as_str()).map(|s| s.to_string());
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }
}

fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

pub struct McpTool {
    spec: ToolSpec,
    client: Arc<McpClient>,
    remote: String,
    read_only: bool,
    timeout: Duration,
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    fn read_only(&self) -> bool {
        self.read_only
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> ToolResult {
        match self.client.request("tools/call", json!({"name": self.remote, "arguments": args}), self.timeout, ctx.cancel.as_ref()).await {
            Err(e) => ToolResult::err(e),
            Ok(r) => {
                let text: Vec<String> = r
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|a| a.iter().map(|c| match c.get("type").and_then(|t| t.as_str()) {
                        Some("text") => c.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
                        Some(t) => format!("[{t} content omitted]"),
                        None => String::new(),
                    }).collect())
                    .unwrap_or_default();
                let out = clip(&text.join("\n"), 30_000);
                if r.get("isError").and_then(|b| b.as_bool()).unwrap_or(false) {
                    ToolResult::err(out)
                } else {
                    ToolResult::ok(out)
                }
            }
        }
    }
}

/// Connect to every configured server and wrap its tools. Returns (tools, notices).
pub async fn load_tools(cwd: &Path, env: &Env) -> (Vec<ToolRef>, Vec<String>) {
    let mut servers: Vec<(String, ServerConfig)> = Vec::new();
    let mut notes = Vec::new();
    if let Ok(s) = std::fs::read_to_string(config_dir(env).join("mcp.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            servers.extend(parse_config(&v, env));
        }
    }
    let project = cwd.join(".fh").join("mcp.json");
    if project.exists() {
        if crate::trust::is_trusted(env, cwd) {
            if let Some(v) = std::fs::read_to_string(&project).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
                servers.extend(parse_config(&v, env));
            }
        } else {
            notes.push("project MCP servers (.fh/mcp.json) were skipped: this workspace is not trusted (run `fh trust`)".to_string());
        }
    }
    let mut tools: Vec<ToolRef> = Vec::new();
    for (name, cfg) in servers {
        match McpClient::connect(&name, &cfg, cwd).await {
            Err(e) => notes.push(e),
            Ok(client) => match client.list_tools().await {
                Err(e) => notes.push(format!("MCP server \"{name}\": {e}")),
                Ok(list) => {
                    notes.push(format!("MCP server \"{name}\": {} tool(s)", list.len()));
                    for t in list {
                        let full: String = format!("mcp__{}__{}", safe(&name), safe(&t.name)).chars().take(64).collect();
                        let desc = format!("[MCP {name}] {}", t.description);
                        tools.push(Arc::new(McpTool { spec: ToolSpec { name: full, description: desc, parameters: t.schema }, client: client.clone(), remote: t.name, read_only: t.read_only, timeout: Duration::from_secs(120) }));
                    }
                }
            },
        }
    }
    (tools, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_shapes_and_expansion() {
        let mut env = Env::new();
        env.insert("TOKEN".into(), "abc".into());
        let v = json!({"mcpServers": {"a": {"command": "srv", "args": ["--k=${TOKEN}"], "env": {"X": "${TOKEN}"}}, "b": {"url": "http://h/mcp", "headers": {"Authorization": "Bearer ${TOKEN}"}}}});
        let p = parse_config(&v, &env);
        assert_eq!(p.len(), 2);
        match &p.iter().find(|(n, _)| n == "a").unwrap().1 {
            ServerConfig::Stdio { args, env, .. } => {
                assert_eq!(args[0], "--k=abc");
                assert_eq!(env["X"], "abc");
            }
            _ => panic!(),
        }
        match &p.iter().find(|(n, _)| n == "b").unwrap().1 {
            ServerConfig::Http { headers, .. } => assert_eq!(headers["Authorization"], "Bearer abc"),
            _ => panic!(),
        }
        assert_eq!(safe("a.b c"), "a_b_c");
    }
}
