//! HTTP client construction (mTLS identity, extra CA) and command-provided access tokens (OIDC/JWT refresh).
use crate::config::Config;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

static TOKENS: LazyLock<Mutex<HashMap<String, (String, Instant)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Runs `cmd` (stdout = token) at most once per `ttl_secs`; the result is cached until it expires or is invalidated.
pub fn token_from_cmd(cmd: &str, ttl_secs: u64) -> Option<String> {
    if let Some((t, at)) = TOKENS.lock().unwrap().get(cmd) {
        if at.elapsed() < Duration::from_secs(ttl_secs) {
            return Some(t.clone());
        }
    }
    let shell = crate::util::proc::shell_for(cmd);
    let mut child = std::process::Command::new(&shell.file).args(&shell.args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(15) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if tok.is_empty() {
        return None;
    }
    TOKENS.lock().unwrap().insert(cmd.to_string(), (tok.clone(), Instant::now()));
    Some(tok)
}

/// Forces the next request to fetch a fresh token (called after a 401).
pub fn invalidate_token(cmd: &str) {
    TOKENS.lock().unwrap().remove(cmd);
}

pub fn cached_tokens() -> Vec<String> {
    TOKENS.lock().unwrap().values().map(|(t, _)| t.clone()).collect()
}

/// A client builder carrying the configured mTLS identity and extra CA.
pub fn client_builder(cfg: &Config) -> Result<reqwest::ClientBuilder, String> {
    let mut b = reqwest::Client::builder();
    if !cfg.client_cert.is_empty() {
        let mut pem = std::fs::read(&cfg.client_cert).map_err(|e| format!("cannot read clientCert {}: {e}", cfg.client_cert))?;
        if !cfg.client_key.is_empty() {
            pem.push(b'\n');
            pem.extend(std::fs::read(&cfg.client_key).map_err(|e| format!("cannot read clientKey {}: {e}", cfg.client_key))?);
        }
        b = b.identity(reqwest::Identity::from_pem(&pem).map_err(|e| format!("invalid client certificate/key: {e}"))?);
    }
    if !cfg.ca_cert.is_empty() {
        let pem = std::fs::read(&cfg.ca_cert).map_err(|e| format!("cannot read caCert {}: {e}", cfg.ca_cert))?;
        b = b.add_root_certificate(reqwest::Certificate::from_pem(&pem).map_err(|e| format!("invalid caCert: {e}"))?);
    }
    Ok(b)
}

/// Client for the configured endpoint; a bad certificate path falls back to a plain client (the request then fails visibly).
pub fn client(cfg: &Config) -> reqwest::Client {
    client_builder(cfg).and_then(|b| b.build().map_err(|e| { let mut m = e.to_string(); let mut src = std::error::Error::source(&e); while let Some(s) = src { m.push_str(&format!(": {s}")); src = s.source(); } m })).unwrap_or_else(|e| {
        eprintln!("warning: {e}");
        reqwest::Client::new()
    })
}
