use super::*;
use crate::types::ToolSpec;
use crate::util::paths::{glob_to_regex, in_workspace, matches_any, rel};
use crate::util::proc::{clip, run, RunOpts};
use regex::RegexBuilder;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

fn hash(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

const IGNORE: [&str; 12] = [".git", "node_modules", "bin", "obj", "dist", "build", ".next", ".venv", "__pycache__", "target", ".fh", "coverage"];

fn check_writable(ctx: &ToolCtx, abs: &Path) -> Result<(), String> {
    let r = rel(&ctx.cwd, abs);
    if r == ".git" || r.starts_with(".git/") {
        return Err("refusing to modify .git".into());
    }
    if let Some(g) = &ctx.owned_globs {
        if !matches_any(&r, g) {
            return Err(format!("file ownership: {r} is not owned by this worker (owned: {})", g.join(", ")));
        }
    }
    Ok(())
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>, max: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= max {
            return;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if IGNORE.contains(&name.as_str()) {
            continue;
        }
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(root, &p, out, max),
            Ok(t) if t.is_file() => out.push(rel(root, &p)),
            _ => {}
        }
    }
}

pub fn list_all(cwd: &Path, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    walk(&root, &root, &mut out, max);
    out
}

fn spec(name: &str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec { name: name.into(), description: description.into(), parameters }
}

pub struct ReadFile(ToolSpec);
impl ReadFile {
    pub fn new() -> Self {
        ReadFile(spec("read_file", "Read a text file. Returns numbered lines. Use offset/limit for large files.", json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","description":"1-based first line"},"limit":{"type":"integer"}},"required":["path"]})))
    }
}
#[async_trait]
impl Tool for ReadFile {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }
    fn read_only(&self) -> bool {
        true
    }
    async fn execute(&self, a: &Value, ctx: &ToolCtx) -> ToolResult {
        let p = match str_arg(a, "path") {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let abs = match in_workspace(&ctx.cwd, p) {
            Ok(x) => x,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        if !abs.exists() {
            return ToolResult::err(format!("not found: {p}"));
        }
        if abs.is_dir() {
            return ToolResult::err("is a directory; use list_files");
        }
        let text = match std::fs::read_to_string(&abs) {
            Ok(t) => t,
            Err(e) => return ToolResult::err(format!("cannot read as text: {e}")),
        };
        ctx.read_cache.lock().unwrap().insert(abs, hash(&text));
        let lines: Vec<&str> = text.split('\n').collect();
        let off = num_arg(a, "offset").unwrap_or(1).max(1) as usize;
        let lim = num_arg(a, "limit").unwrap_or(400).clamp(1, 2000) as usize;
        let slice: Vec<String> = lines.iter().skip(off - 1).take(lim).enumerate().map(|(i, l)| format!("{}\t{}", off + i, l)).collect();
        let more = if off - 1 + lim < lines.len() { format!("\n… {} more lines (use offset)", lines.len() - (off - 1 + lim)) } else { String::new() };
        ToolResult::ok(format!("{}{}", clip(&slice.join("\n"), 12000), more))
    }
}

pub struct ListFiles(ToolSpec);
impl ListFiles {
    pub fn new() -> Self {
        ListFiles(spec("list_files", "List files (recursive, skips node_modules/.git/build dirs). Optional glob like \"src/**/*.ts\".", json!({"type":"object","properties":{"glob":{"type":"string"},"dir":{"type":"string"}}})))
    }
}
#[async_trait]
impl Tool for ListFiles {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }
    fn read_only(&self) -> bool {
        true
    }
    async fn execute(&self, a: &Value, ctx: &ToolCtx) -> ToolResult {
        let base = match opt_str(a, "dir") {
            "" => ctx.cwd.clone(),
            d => match in_workspace(&ctx.cwd, d) {
                Ok(p) => p,
                Err(e) => return ToolResult::err(e.to_string()),
            },
        };
        let root = ctx.cwd.canonicalize().unwrap_or_else(|_| ctx.cwd.clone());
        let mut out = Vec::new();
        walk(&root, &base.canonicalize().unwrap_or(base), &mut out, 5000);
        let g = opt_str(a, "glob");
        let files: Vec<String> = if g.is_empty() { out } else { let re = glob_to_regex(g); out.into_iter().filter(|f| re.is_match(f)).collect() };
        if files.is_empty() {
            return ToolResult::ok("(no files)");
        }
        let more = if files.len() > 500 { format!("\n… {} more", files.len() - 500) } else { String::new() };
        ToolResult::ok(format!("{}{}", clip(&files.iter().take(500).cloned().collect::<Vec<_>>().join("\n"), 12000), more))
    }
}

pub struct Grep(ToolSpec);
impl Grep {
    pub fn new() -> Self {
        Grep(spec("grep", "Search file contents with a regex. Returns path:line:text. Optional glob filter.", json!({"type":"object","properties":{"pattern":{"type":"string"},"glob":{"type":"string"},"ignore_case":{"type":"boolean"}},"required":["pattern"]})))
    }
}
#[async_trait]
impl Tool for Grep {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }
    fn read_only(&self) -> bool {
        true
    }
    async fn execute(&self, a: &Value, ctx: &ToolCtx) -> ToolResult {
        let pat = match str_arg(a, "pattern") {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let re = match RegexBuilder::new(pat).case_insensitive(a.get("ignore_case").and_then(|v| v.as_bool()).unwrap_or(false)).build() {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("bad regex: {e}")),
        };
        let g = opt_str(a, "glob");
        let gre = if g.is_empty() { None } else { Some(glob_to_regex(g)) };
        let mut hits: Vec<String> = Vec::new();
        let root = ctx.cwd.canonicalize().unwrap_or_else(|_| ctx.cwd.clone());
        'files: for f in list_all(&ctx.cwd, 5000) {
            if gre.as_ref().map(|r| !r.is_match(&f)).unwrap_or(false) {
                continue;
            }
            let p = root.join(&f);
            if std::fs::metadata(&p).map(|m| m.len() > 1_000_000).unwrap_or(true) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            if text.contains('\0') {
                continue;
            }
            for (i, l) in text.lines().enumerate() {
                if re.is_match(l) {
                    hits.push(format!("{}:{}:{}", f, i + 1, l.chars().take(240).collect::<String>()));
                    if hits.len() >= 200 {
                        break 'files;
                    }
                }
            }
        }
        ToolResult::ok(if hits.is_empty() { "(no matches)".to_string() } else { hits.join("\n") })
    }
}

/// Whitespace-tolerant locate: exact first, then line-trimmed. Returns (start, end, count, fuzzy) as byte offsets.
pub fn locate(text: &str, old: &str) -> Option<(usize, usize, usize, bool)> {
    let count = text.matches(old).count();
    if count >= 1 {
        let start = text.find(old).unwrap();
        return Some((start, start + old.len(), count, false));
    }
    let tl: Vec<&str> = text.split('\n').collect();
    let old_trim = old.trim_end_matches('\n');
    let ol: Vec<&str> = old_trim.split('\n').collect();
    let mut matches = Vec::new();
    if ol.len() <= tl.len() {
        for i in 0..=(tl.len() - ol.len()) {
            if (0..ol.len()).all(|j| tl[i + j].trim() == ol[j].trim()) {
                matches.push(i);
            }
        }
    }
    let i = *matches.first()?;
    let start: usize = tl[..i].iter().map(|l| l.len() + 1).sum();
    let end = start + tl[i..i + ol.len()].join("\n").len();
    Some((start, end, matches.len(), true))
}

pub struct Edit(ToolSpec);
impl Edit {
    pub fn new() -> Self {
        Edit(spec("edit", "Replace one exact snippet in an existing file (search/replace). old_text must match exactly once; include enough context. Prefer this over rewriting files.", json!({"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_text","new_text"]})))
    }
}
#[async_trait]
impl Tool for Edit {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }
    fn read_only(&self) -> bool {
        false
    }
    async fn execute(&self, a: &Value, ctx: &ToolCtx) -> ToolResult {
        let (p, old_t, new_t) = match (str_arg(a, "path"), str_arg(a, "old_text"), str_arg(a, "new_text")) {
            (Ok(p), Ok(o), Ok(n)) => (p, o, n),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return ToolResult::err(e),
        };
        let abs = match in_workspace(&ctx.cwd, p) {
            Ok(x) => x,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        if let Err(e) = check_writable(ctx, &abs) {
            return ToolResult::err(e);
        }
        if !abs.exists() {
            return ToolResult::err(format!("not found: {p} (use write_file to create)"));
        }
        let text = match std::fs::read_to_string(&abs) {
            Ok(t) => t,
            Err(e) => return ToolResult::err(format!("cannot read as text: {e}")),
        };
        if let Some(prev) = ctx.read_cache.lock().unwrap().get(&abs) {
            if *prev != hash(&text) {
                return ToolResult::err("file changed since you read it; read_file again");
            }
        }
        if old_t == new_t {
            return ToolResult::err("old_text and new_text are identical");
        }
        if old_t.is_empty() {
            return ToolResult::err("old_text is empty");
        }
        let out = if a.get("replace_all").and_then(|v| v.as_bool()) == Some(true) && text.contains(old_t) {
            text.replace(old_t, new_t)
        } else {
            let Some((s, e, count, _)) = locate(&text, old_t) else {
                return ToolResult::err("old_text not found. Re-read the file and copy the snippet exactly.");
            };
            if count > 1 {
                return ToolResult::err(format!("old_text matches {count} places; add surrounding lines to make it unique (or set replace_all)."));
            }
            format!("{}{}{}", &text[..s], new_t, &text[e..])
        };
        if let Err(e) = std::fs::write(&abs, &out) {
            return ToolResult::err(format!("write failed: {e}"));
        }
        ctx.read_cache.lock().unwrap().insert(abs.clone(), hash(&out));
        let r = rel(&ctx.cwd, &abs);
        ctx.touched.lock().unwrap().insert(r.clone());
        ToolResult::ok(format!("edited {r}"))
    }
}

pub struct WriteNew(ToolSpec);
impl WriteNew {
    pub fn new() -> Self {
        WriteNew(spec("write_file", "Create a NEW file. Fails if the file exists (use edit instead).", json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]})))
    }
}
#[async_trait]
impl Tool for WriteNew {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }
    fn read_only(&self) -> bool {
        false
    }
    async fn execute(&self, a: &Value, ctx: &ToolCtx) -> ToolResult {
        let (p, content) = match (str_arg(a, "path"), str_arg(a, "content")) {
            (Ok(p), Ok(c)) => (p, c),
            (Err(e), _) | (_, Err(e)) => return ToolResult::err(e),
        };
        let abs = match in_workspace(&ctx.cwd, p) {
            Ok(x) => x,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        if let Err(e) = check_writable(ctx, &abs) {
            return ToolResult::err(e);
        }
        if abs.exists() {
            return ToolResult::err("file exists; use edit for changes");
        }
        if let Some(parent) = abs.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&abs, content) {
            return ToolResult::err(format!("write failed: {e}"));
        }
        let r = rel(&ctx.cwd, &abs);
        ctx.touched.lock().unwrap().insert(r.clone());
        ToolResult::ok(format!("created {r}"))
    }
}

pub struct Bash(ToolSpec);
impl Bash {
    pub fn new() -> Self {
        Bash(spec("bash", "Run a shell command in the workspace (bash; PowerShell on Windows). Use for builds, tests, git. Output is truncated.", json!({"type":"object","properties":{"command":{"type":"string"},"timeout_s":{"type":"integer"}},"required":["command"]})))
    }
}
#[async_trait]
impl Tool for Bash {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }
    fn read_only(&self) -> bool {
        false
    }
    async fn execute(&self, a: &Value, ctx: &ToolCtx) -> ToolResult {
        let cmd = match str_arg(a, "command") {
            Ok(c) => c,
            Err(e) => return ToolResult::err(e),
        };
        let t_ms = (num_arg(a, "timeout_s").unwrap_or(120).max(1) as u64 * 1000).min(ctx.bash_timeout_ms);
        let r = run(cmd, &ctx.cwd, RunOpts { timeout: Some(Duration::from_millis(t_ms)), cancel: ctx.cancel.clone(), env: None, wrap: ctx.wrap_shell.clone() }).await;
        let mut body = r.stdout.clone();
        if !r.stderr.is_empty() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&format!("[stderr]\n{}", r.stderr));
        }
        let body = clip(&body, 12000);
        let tag = if r.aborted { " (cancelled)".to_string() } else if r.timed_out { format!(" (timed out after {}s)", t_ms / 1000) } else { String::new() };
        let code = r.code.map(|c| c.to_string()).unwrap_or_else(|| "none".into());
        ToolResult { ok: r.code == Some(0) && !r.timed_out && !r.aborted, output: format!("exit {code}{tag}\n{body}").trim().to_string() }
    }
}
