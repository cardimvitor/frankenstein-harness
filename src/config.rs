use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub type Env = HashMap<String, String>;

pub fn process_env() -> Env {
    std::env::vars().collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sampling {
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: u32,
    pub presence_penalty: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplingSet {
    pub thinking: Sampling,
    pub instant: Sampling,
}

impl Default for Sampling {
    fn default() -> Self {
        Sampling { temperature: 0.6, top_p: 0.95, top_k: 20, presence_penalty: None }
    }
}

impl Default for SamplingSet {
    fn default() -> Self {
        // Qwen3-family recommended defaults; re-tune with `fh validate-vllm` on the real model.
        SamplingSet {
            thinking: Sampling { temperature: 0.6, top_p: 0.95, top_k: 20, presence_penalty: None },
            instant: Sampling { temperature: 0.7, top_p: 0.8, top_k: 20, presence_penalty: None },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub endpoint: String,
    pub model: String,
    /// bearer | header | none
    pub auth_scheme: String,
    pub auth_header: String,
    /// name of the env var holding the secret (the secret itself is never stored)
    pub api_key_env: String,
    pub extra_headers: HashMap<String, String>,
    pub client_id: String,
    pub metrics_url: String,
    pub context_window: usize,
    pub max_output_tokens: u32,
    pub max_steps: usize,
    pub request_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub retries: u32,
    pub max_concurrency: usize,
    pub verify_rounds_normal: usize,
    pub verify_rounds_auto: usize,
    /// total prompt+completion tokens one task may use across all rounds and workers (0 = unlimited)
    pub max_task_tokens: u64,
    /// use language servers as a verifier input (kept warm between rounds)
    pub lsp_diagnostics: bool,
    /// optional embedding model for skill recall (empty = BM25 only); served at `embeddingEndpoint` or the main endpoint
    pub embedding_model: String,
    pub embedding_endpoint: String,
    /// summarize old steps with one extra thinking-off call when the context fills up
    pub llm_compaction: bool,
    /// where the API key comes from when the env var is unset: auto | env | keychain
    pub api_key_store: String,
    /// mTLS: PEM file with the client certificate (and, unless `clientKey` is set, its private key)
    pub client_cert: String,
    pub client_key: String,
    /// extra CA certificate (PEM) to trust for the endpoint
    pub ca_cert: String,
    /// command that prints a fresh access token (OIDC/JWT: e.g. a CLI that refreshes it); cached, refreshed on 401
    pub auth_token_cmd: String,
    pub auth_token_ttl_secs: u64,
    /// opt-in local JSONL of task outcomes and timings (never leaves the machine)
    pub telemetry: bool,
    /// "use" (default) or "off": with "off" learned skills are still created and improved, but never put in the prompt
    pub memory: String,
    /// max-parallel mode: read-only scouts and/or file-partitioned workers run in parallel, then ONE agent consolidates
    /// every finding and solution (needs maxConcurrency > 1)
    pub consolidate: bool,
    pub sampling: SamplingSet,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            endpoint: "http://127.0.0.1:8000/v1".into(),
            model: "qwen".into(),
            auth_scheme: "bearer".into(),
            auth_header: "Authorization".into(),
            api_key_env: "FH_API_KEY".into(),
            extra_headers: HashMap::new(),
            client_id: "frankenstein-harness".into(),
            metrics_url: String::new(),
            context_window: 131072,
            max_output_tokens: 8192,
            max_steps: 40,
            request_timeout_ms: 600_000,
            idle_timeout_ms: 120_000,
            retries: 3,
            max_concurrency: 3,
            verify_rounds_normal: 2,
            verify_rounds_auto: 5,
            max_task_tokens: 800_000,
            lsp_diagnostics: true,
            embedding_model: String::new(),
            embedding_endpoint: String::new(),
            llm_compaction: true,
            api_key_store: "auto".into(),
            telemetry: false,
            memory: "use".into(),
            consolidate: false,
            client_cert: String::new(),
            client_key: String::new(),
            ca_cert: String::new(),
            auth_token_cmd: String::new(),
            auth_token_ttl_secs: 300,
            sampling: SamplingSet::default(),
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// Per-user data dir: XDG on Linux, ~/Library/Application Support on macOS, %LOCALAPPDATA% on Windows.
pub fn data_dir(env: &Env) -> PathBuf {
    if let Some(p) = env.get("FH_HOME") {
        return PathBuf::from(p);
    }
    if cfg!(windows) {
        let base = env.get("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData").join("Local"));
        base.join("frankenstein-harness")
    } else if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support").join("frankenstein-harness")
    } else {
        env.get("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local").join("share")).join("frankenstein-harness")
    }
}

pub fn config_dir(env: &Env) -> PathBuf {
    if let Some(p) = env.get("FH_CONFIG_HOME") {
        return PathBuf::from(p);
    }
    if cfg!(windows) {
        env.get("APPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData").join("Roaming")).join("frankenstein-harness")
    } else if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support").join("frankenstein-harness")
    } else {
        env.get("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("frankenstein-harness")
    }
}

fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (b, o) => *b = o.clone(),
    }
}

fn read_json(p: &Path) -> Result<Value> {
    if !p.exists() {
        return Ok(Value::Object(Default::default()));
    }
    let s = std::fs::read_to_string(p)?;
    serde_json::from_str(&s).map_err(|e| anyhow!("Invalid config {}: {e}", p.display()))
}

/// defaults < user config file < project .fh/config.json < environment
pub fn load_config(cwd: &Path, env: &Env) -> Result<Config> {
    let mut v = serde_json::to_value(Config::default())?;
    merge(&mut v, &read_json(&config_dir(env).join("config.json"))?);
    merge(&mut v, &read_json(&cwd.join(".fh").join("config.json"))?);
    let mut cfg: Config = serde_json::from_value(v)?;
    if let Some(x) = env.get("FH_ENDPOINT") {
        cfg.endpoint = x.clone();
    }
    if let Some(x) = env.get("FH_MODEL") {
        cfg.model = x.clone();
    }
    if let Some(x) = env.get("FH_AUTH_SCHEME") {
        cfg.auth_scheme = x.clone();
    }
    if let Some(x) = env.get("FH_API_KEY_ENV") {
        cfg.api_key_env = x.clone();
    }
    if let Some(x) = env.get("FH_METRICS_URL") {
        cfg.metrics_url = x.clone();
    }
    if let Some(x) = env.get("FH_CONTEXT_WINDOW").and_then(|s| s.parse().ok()) {
        cfg.context_window = x;
    }
    if let Some(x) = env.get("FH_MAX_CONCURRENCY").and_then(|s| s.parse().ok()) {
        cfg.max_concurrency = x;
    }
    if let Some(x) = env.get("FH_MEMORY") {
        cfg.memory = if x.eq_ignore_ascii_case("off") || x == "0" || x.eq_ignore_ascii_case("none") { "off".into() } else { "use".into() };
    }
    if let Some(x) = env.get("FH_CONSOLIDATE") {
        cfg.consolidate = x == "1" || x.eq_ignore_ascii_case("true");
    }
    cfg.endpoint = cfg.endpoint.trim_end_matches('/').to_string();
    if cfg.metrics_url.is_empty() {
        cfg.metrics_url = format!("{}/metrics", origin_of(&cfg.endpoint));
    }
    Ok(cfg)
}

/// scheme://host[:port] of a URL.
pub fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let host = rest.split('/').next().unwrap_or(rest);
    format!("{scheme}://{host}")
}

pub fn auth_headers(cfg: &Config, env: &Env) -> Vec<(String, String)> {
    let mut h = vec![("x-client-id".to_string(), cfg.client_id.clone())];
    for (k, v) in &cfg.extra_headers {
        h.push((k.clone(), v.clone()));
    }
    let dynamic = if cfg.auth_token_cmd.is_empty() { None } else { crate::http::token_from_cmd(&cfg.auth_token_cmd, cfg.auth_token_ttl_secs) };
    if let Some(secret) = dynamic.as_ref().or_else(|| env.get(&cfg.api_key_env).filter(|s| !s.is_empty())) {
        match cfg.auth_scheme.as_str() {
            "bearer" => h.push(("Authorization".into(), format!("Bearer {secret}"))),
            "header" => h.push((cfg.auth_header.clone(), secret.clone())),
            _ => {}
        }
    }
    h
}

fn secret_values(env: &Env) -> Vec<String> {
    let re = |k: &str| {
        let k = k.to_lowercase();
        ["key", "token", "secret", "password", "auth"].iter().any(|w| k.contains(w))
    };
    let mut v: Vec<String> = env.iter().filter(|(k, val)| val.len() >= 8 && re(k)).map(|(_, val)| val.clone()).collect();
    v.extend(crate::http::cached_tokens().into_iter().filter(|t| t.len() >= 8));
    v.sort_by_key(|s| std::cmp::Reverse(s.len()));
    v
}

/// Redact secrets (values of key/token/secret/password/auth env vars) from any text bound for logs or the UI.
pub fn redact(text: &str, env: &Env) -> String {
    let mut out = text.to_string();
    for s in secret_values(env) {
        out = out.replace(&s, "«redacted»");
    }
    out
}

/// Redacts a text stream where a secret may be split across chunks: holds back the last
/// (longest secret - 1) characters until the next chunk or flush.
pub struct StreamRedactor {
    buf: String,
    env: Env,
}

impl StreamRedactor {
    pub fn new(env: Env) -> Self {
        StreamRedactor { buf: String::new(), env }
    }
    fn hold(&self) -> usize {
        secret_values(&self.env).iter().map(|s| s.chars().count().saturating_sub(1)).max().unwrap_or(0)
    }
    pub fn push(&mut self, chunk: &str) -> String {
        let joined = format!("{}{}", self.buf, chunk);
        self.buf = redact(&joined, &self.env);
        let hold = self.hold();
        let n = self.buf.chars().count();
        if n <= hold {
            return String::new();
        }
        let cut = self.buf.char_indices().nth(n - hold).map(|(i, _)| i).unwrap_or(self.buf.len());
        let out = self.buf[..cut].to_string();
        self.buf = self.buf[cut..].to_string();
        out
    }
    pub fn flush(&mut self) -> String {
        let out = redact(&self.buf, &self.env);
        self.buf.clear();
        out
    }
}
