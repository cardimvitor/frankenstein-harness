//! OAuth 2.1 for remote MCP servers: protected-resource discovery (RFC 9728), authorization-server metadata
//! (RFC 8414), dynamic client registration (RFC 7591), authorization code + PKCE S256, resource indicators
//! (RFC 8707), refresh tokens. `fh mcp login <server>` runs the browser flow once; afterwards requests carry the
//! access token and refresh it automatically. Tokens live in `<data dir>/mcp-oauth.json` (mode 0600 on Unix).
use base64::Engine as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoredAuth {
    pub server_url: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// unix seconds; 0 = unknown (treated as valid until the server answers 401)
    #[serde(default)]
    pub expires_at: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Per-server token file.
pub struct OAuthStore {
    path: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

impl OAuthStore {
    pub fn new(path: PathBuf) -> Arc<OAuthStore> {
        Arc::new(OAuthStore { path, lock: tokio::sync::Mutex::new(()) })
    }

    fn read_all(&self) -> HashMap<String, StoredAuth> {
        std::fs::read_to_string(&self.path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn get(&self, server: &str) -> Option<StoredAuth> {
        self.read_all().remove(server)
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.read_all().into_keys().collect();
        v.sort();
        v
    }

    pub fn put(&self, server: &str, a: StoredAuth) -> std::io::Result<()> {
        let mut all = self.read_all();
        all.insert(server.to_string(), a);
        self.write_all(&all)
    }

    pub fn remove(&self, server: &str) -> std::io::Result<bool> {
        let mut all = self.read_all();
        let had = all.remove(server).is_some();
        self.write_all(&all)?;
        Ok(had)
    }

    fn write_all(&self, all: &HashMap<String, StoredAuth>) -> std::io::Result<()> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(all).unwrap_or_default())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(tmp, &self.path)
    }
}

/// Live token for one server: hands out a valid access token, refreshing it when it is about to expire.
pub struct OAuthSession {
    pub server: String,
    store: Arc<OAuthStore>,
    http: reqwest::Client,
}

impl OAuthSession {
    pub fn new(server: &str, store: Arc<OAuthStore>) -> Option<Arc<OAuthSession>> {
        store.get(server)?;
        Some(Arc::new(OAuthSession { server: server.to_string(), store, http: reqwest::Client::new() }))
    }

    /// A usable access token (refreshed if it expires within a minute).
    pub async fn access_token(&self) -> Result<String, String> {
        let _g = self.store.lock.lock().await;
        let a = self.store.get(&self.server).ok_or_else(|| format!("not logged in: run `fh mcp login {}`", self.server))?;
        if a.expires_at != 0 && a.expires_at <= now() + 60 {
            return self.refresh_locked(a).await;
        }
        Ok(a.access_token)
    }

    /// Forces a refresh (after a 401).
    pub async fn refresh(&self) -> Result<String, String> {
        let _g = self.store.lock.lock().await;
        let a = self.store.get(&self.server).ok_or_else(|| format!("not logged in: run `fh mcp login {}`", self.server))?;
        self.refresh_locked(a).await
    }

    async fn refresh_locked(&self, mut a: StoredAuth) -> Result<String, String> {
        let Some(rt) = a.refresh_token.clone() else { return Err(format!("the access token expired and there is no refresh token: run `fh mcp login {}`", self.server)) };
        let mut form = vec![("grant_type", "refresh_token".to_string()), ("refresh_token", rt), ("client_id", a.client_id.clone()), ("resource", a.server_url.clone())];
        if let Some(s) = &a.client_secret {
            form.push(("client_secret", s.clone()));
        }
        let res = self.http.post(&a.token_endpoint).header("accept", "application/json").form(&form).timeout(Duration::from_secs(30)).send().await.map_err(|e| format!("token refresh failed: {e}"))?;
        if !res.status().is_success() {
            return Err(format!("token refresh rejected (HTTP {}): run `fh mcp login {}`", res.status().as_u16(), self.server));
        }
        let j: Value = res.json().await.map_err(|e| e.to_string())?;
        apply_token_response(&mut a, &j)?;
        let tok = a.access_token.clone();
        self.store.put(&self.server, a).map_err(|e| e.to_string())?;
        Ok(tok)
    }
}

fn apply_token_response(a: &mut StoredAuth, j: &Value) -> Result<(), String> {
    a.access_token = j["access_token"].as_str().ok_or("token response has no access_token")?.to_string();
    if let Some(r) = j["refresh_token"].as_str() {
        a.refresh_token = Some(r.to_string());
    }
    a.expires_at = j["expires_in"].as_u64().map(|e| now() + e).unwrap_or(0);
    Ok(())
}

fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn random_token(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b64url(&b)
}

pub fn pkce_challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

fn origin(url: &str) -> String {
    crate::config::origin_of(url)
}

/// `resource_metadata="<url>"` from a `WWW-Authenticate: Bearer ...` header.
fn resource_metadata_url(www: &str) -> Option<String> {
    let i = www.find("resource_metadata=")? + "resource_metadata=".len();
    let rest = &www[i..];
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    Some(rest.split(|c| c == '"' || c == ',' || c == ' ').next()?.to_string())
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<Value, String> {
    let r = http.get(url).header("accept", "application/json").timeout(Duration::from_secs(20)).send().await.map_err(|e| format!("{url}: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("{url}: HTTP {}", r.status().as_u16()));
    }
    r.json().await.map_err(|e| format!("{url}: {e}"))
}

#[derive(Debug)]
pub struct Discovery {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
}

/// Finds the authorization server for an MCP server URL and reads its metadata.
pub async fn discover(http: &reqwest::Client, server_url: &str, extra_headers: &HashMap<String, String>) -> Result<Discovery, String> {
    // an unauthenticated request tells us where the resource metadata lives
    let mut req = http.post(server_url).header("content-type", "application/json").header("accept", "application/json, text/event-stream").body(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string()).timeout(Duration::from_secs(20));
    for (k, v) in extra_headers {
        req = req.header(k, v);
    }
    let probe = req.send().await.map_err(|e| format!("cannot reach {server_url}: {e}"))?;
    let www = probe.headers().get("www-authenticate").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
    let prm_url = resource_metadata_url(&www).unwrap_or_else(|| format!("{}/.well-known/oauth-protected-resource", origin(server_url)));
    let prm = get_json(http, &prm_url).await;
    let issuer = match &prm {
        Ok(p) => p["authorization_servers"].as_array().and_then(|a| a.first()).and_then(|s| s.as_str()).map(|s| s.trim_end_matches('/').to_string()).unwrap_or_else(|| origin(server_url)),
        Err(_) => origin(server_url), // older servers act as their own authorization server
    };
    let meta = match get_json(http, &format!("{issuer}/.well-known/oauth-authorization-server")).await {
        Ok(m) => m,
        Err(_) => get_json(http, &format!("{issuer}/.well-known/openid-configuration")).await.map_err(|e| format!("no authorization server metadata for {issuer}: {e}"))?,
    };
    let g = |k: &str| meta[k].as_str().map(|s| s.to_string());
    let (Some(authorization_endpoint), Some(token_endpoint)) = (g("authorization_endpoint"), g("token_endpoint")) else { return Err(format!("{issuer} metadata lacks authorization_endpoint or token_endpoint")) };
    if let Some(methods) = meta["code_challenge_methods_supported"].as_array() {
        if !methods.iter().any(|m| m == "S256") {
            return Err(format!("{issuer} does not support PKCE S256"));
        }
    }
    Ok(Discovery { authorization_endpoint, token_endpoint, registration_endpoint: g("registration_endpoint") })
}

/// Waits for the browser redirect on a loopback port and returns (code, state).
async fn wait_for_callback(listener: tokio::net::TcpListener, timeout: Duration) -> Result<(String, String), String> {
    let accept = async {
        loop {
            let (mut sock, _) = listener.accept().await.map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 8192];
            let n = tokio::time::timeout(Duration::from_secs(10), sock.read(&mut buf)).await.ok().and_then(|r| r.ok()).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let target = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("").to_string();
            if !target.starts_with("/callback") {
                let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
                continue;
            }
            let q: HashMap<String, String> = url::form_urlencoded::parse(target.split_once('?').map(|x| x.1).unwrap_or("").as_bytes()).into_owned().collect();
            let body = if q.contains_key("code") { "Signed in. You can close this tab and return to the terminal." } else { "Sign-in failed. Return to the terminal." };
            let _ = sock.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
            if let Some(e) = q.get("error") {
                return Err(format!("authorization refused: {e} {}", q.get("error_description").cloned().unwrap_or_default()));
            }
            return match (q.get("code"), q.get("state")) {
                (Some(c), Some(s)) => Ok((c.clone(), s.clone())),
                _ => Err("callback without code and state".to_string()),
            };
        }
    };
    tokio::time::timeout(timeout, accept).await.map_err(|_| "timed out waiting for the browser sign-in".to_string())?
}

/// Runs the full browser flow. `open_url` shows the URL to the user (opens a browser, or prints it).
pub async fn login(server: &str, server_url: &str, extra_headers: &HashMap<String, String>, store: &OAuthStore, open_url: Arc<dyn Fn(&str) + Send + Sync>, timeout: Duration) -> Result<(), String> {
    let http = reqwest::Client::new();
    let d = discover(&http, server_url, extra_headers).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
    let redirect = format!("http://127.0.0.1:{}/callback", listener.local_addr().map_err(|e| e.to_string())?.port());
    // reuse the registration from an earlier login when the redirect still fits (loopback ports may differ, so register per login)
    let (client_id, client_secret) = match &d.registration_endpoint {
        Some(reg) => {
            let r = http.post(reg).json(&json!({"client_name": "Frankenstein Harness", "redirect_uris": [redirect], "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"], "token_endpoint_auth_method": "none"})).timeout(Duration::from_secs(20)).send().await.map_err(|e| format!("client registration failed: {e}"))?;
            if !r.status().is_success() {
                return Err(format!("client registration rejected (HTTP {})", r.status().as_u16()));
            }
            let j: Value = r.json().await.map_err(|e| e.to_string())?;
            (j["client_id"].as_str().ok_or("registration returned no client_id")?.to_string(), j["client_secret"].as_str().map(|s| s.to_string()))
        }
        None => return Err("the server offers no dynamic client registration; register a client with it and add `oauthClientId` to the config (not supported yet)".into()),
    };
    let verifier = random_token(48);
    let state = random_token(24);
    let mut auth = url::Url::parse(&d.authorization_endpoint).map_err(|e| e.to_string())?;
    auth.query_pairs_mut().append_pair("response_type", "code").append_pair("client_id", &client_id).append_pair("redirect_uri", &redirect).append_pair("code_challenge", &pkce_challenge(&verifier)).append_pair("code_challenge_method", "S256").append_pair("state", &state).append_pair("resource", server_url);
    open_url(auth.as_str());
    let (code, got_state) = wait_for_callback(listener, timeout).await?;
    if got_state != state {
        return Err("sign-in state mismatch: the response did not come from this login attempt".into());
    }
    let mut form = vec![("grant_type", "authorization_code".to_string()), ("code", code), ("redirect_uri", redirect), ("client_id", client_id.clone()), ("code_verifier", verifier), ("resource", server_url.to_string())];
    if let Some(s) = &client_secret {
        form.push(("client_secret", s.clone()));
    }
    let r = http.post(&d.token_endpoint).header("accept", "application/json").form(&form).timeout(Duration::from_secs(30)).send().await.map_err(|e| format!("token exchange failed: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("token exchange rejected (HTTP {})", r.status().as_u16()));
    }
    let j: Value = r.json().await.map_err(|e| e.to_string())?;
    let mut a = StoredAuth { server_url: server_url.to_string(), client_id, client_secret, token_endpoint: d.token_endpoint, ..Default::default() };
    apply_token_response(&mut a, &j)?;
    store.put(server, a).map_err(|e| e.to_string())
}
