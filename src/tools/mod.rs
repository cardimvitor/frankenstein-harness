use crate::types::ToolSpec;
use crate::util::proc::ShellWrap;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub mod fs;

pub struct ToolCtx {
    pub cwd: PathBuf,
    pub cancel: Option<CancellationToken>,
    /// When set, writes are only allowed to paths matching one of these globs (worker file ownership).
    pub owned_globs: Option<Vec<String>>,
    /// Files read this session -> content hash (edit requires the file not to have changed since the read).
    pub read_cache: Mutex<HashMap<PathBuf, String>>,
    /// Workspace-relative paths modified this session.
    pub touched: Mutex<BTreeSet<String>>,
    pub wrap_shell: Option<ShellWrap>,
    pub bash_timeout_ms: u64,
}

impl ToolCtx {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        ToolCtx { cwd: cwd.into(), cancel: None, owned_globs: None, read_cache: Mutex::new(HashMap::new()), touched: Mutex::new(BTreeSet::new()), wrap_shell: None, bash_timeout_ms: 600_000 }
    }
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
}

impl ToolResult {
    pub fn ok(s: impl Into<String>) -> Self {
        ToolResult { ok: true, output: s.into() }
    }
    pub fn err(s: impl Into<String>) -> Self {
        ToolResult { ok: false, output: s.into() }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> &ToolSpec;
    /// true when the tool never mutates the workspace (safe to run in parallel, allowed in plan mode)
    fn read_only(&self) -> bool;
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> ToolResult;
}

pub type ToolRef = Arc<dyn Tool>;

pub fn all_tools() -> Vec<ToolRef> {
    vec![
        Arc::new(fs::ReadFile::new()),
        Arc::new(fs::ListFiles::new()),
        Arc::new(fs::Grep::new()),
        Arc::new(fs::Edit::new()),
        Arc::new(fs::WriteNew::new()),
        Arc::new(fs::Bash::new()),
    ]
}

pub fn str_arg<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(|v| v.as_str()).ok_or_else(|| format!("missing or non-string argument: {k}"))
}

pub fn opt_str<'a>(a: &'a Value, k: &str) -> &'a str {
    a.get(k).and_then(|v| v.as_str()).unwrap_or("")
}

pub fn num_arg(a: &Value, k: &str) -> Option<i64> {
    match a.get(k) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}
