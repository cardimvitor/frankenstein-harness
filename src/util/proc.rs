use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default)]
pub struct RunResult {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub aborted: bool,
    pub ms: u64,
}

#[derive(Clone, Debug)]
pub struct Shell {
    pub file: String,
    pub args: Vec<String>,
}

pub type ShellWrap = Arc<dyn Fn(Shell) -> Shell + Send + Sync>;

/// The Python 3 interpreter command: `python3` on Unix; on Windows `python` (or the `py -3` launcher), because
/// `python3.exe` there is often the Microsoft Store stub that opens the Store instead of running.
pub fn python() -> &'static str {
    static P: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        if !cfg!(windows) {
            return "python3";
        }
        let ok = |prog: &str, args: &[&str]| std::process::Command::new(prog).args(args).arg("--version").stdin(Stdio::null()).output().map(|o| o.status.success() && String::from_utf8_lossy(&[o.stdout, o.stderr].concat()).contains("Python 3")).unwrap_or(false);
        if ok("python", &[]) { "python" } else if ok("py", &["-3"]) { "py -3" } else { "python" }
    })
}

/// PowerShell 7 (`pwsh`) when installed (UTF-8 by default, `&&` works), Windows PowerShell 5.1 otherwise.
#[cfg(windows)]
fn powershell() -> &'static str {
    static P: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    P.get_or_init(|| if std::process::Command::new("pwsh").arg("-NoProfile").arg("-Command").arg("exit 0").stdin(Stdio::null()).output().map(|o| o.status.success()).unwrap_or(false) { "pwsh.exe" } else { "powershell.exe" })
}

pub fn shell_for(cmd: &str) -> Shell {
    if cfg!(windows) {
        #[cfg(windows)]
        let exe = powershell();
        #[cfg(not(windows))]
        let exe = "powershell.exe";
        Shell { file: exe.into(), args: vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), cmd.into()] }
    } else {
        Shell { file: "bash".into(), args: vec!["-c".into(), cmd.into()] }
    }
}

const SECRET_WORDS: [&str; 7] = ["key", "token", "secret", "password", "passwd", "credential", "auth"];

/// Environment for child processes: secret-looking variables removed.
pub fn scrubbed_env() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(k, _)| {
            // NODE_TEST_CONTEXT makes a nested `node --test` report to a parent runner and always exit 0.
            if k == "NODE_TEST_CONTEXT" {
                return false;
            }
            if k.starts_with("GIT_CONFIG_") {
                return true;
            }
            let l = k.to_lowercase();
            !SECRET_WORDS.iter().any(|w| l.contains(w))
        })
        .collect()
}

/// Truncate keeping head and tail so errors at the end survive.
pub fn clip(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let h = max * 4 / 10;
    let t = max - h;
    format!("{}\n… [{} chars omitted] …\n{}", chars[..h].iter().collect::<String>(), chars.len() - max, chars[chars.len() - t..].iter().collect::<String>())
}

pub fn kill_tree(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
}

#[derive(Default)]
pub struct RunOpts {
    pub timeout: Option<Duration>,
    pub cancel: Option<CancellationToken>,
    /// full environment for the child; None = scrubbed process environment
    pub env: Option<Vec<(String, String)>>,
    pub wrap: Option<ShellWrap>,
    /// written to the child's stdin, then closed
    pub stdin: Option<String>,
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(mut r: R) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < 4_000_000 {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

pub async fn run(cmd: &str, cwd: &Path, o: RunOpts) -> RunResult {
    let t0 = Instant::now();
    let mut sh = shell_for(cmd);
    if let Some(w) = &o.wrap {
        sh = w(sh);
    }
    let mut c = Command::new(&sh.file);
    c.args(&sh.args).current_dir(cwd).stdin(if o.stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    c.env_clear();
    for (k, v) in o.env.unwrap_or_else(scrubbed_env) {
        c.env(k, v);
    }
    #[cfg(unix)]
    c.process_group(0);
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return RunResult { code: Some(-1), stderr: e.to_string(), ms: t0.elapsed().as_millis() as u64, ..Default::default() },
    };
    if let (Some(input), Some(mut si)) = (o.stdin.clone(), child.stdin.take()) {
        tokio::spawn(async move {
            let _ = si.write_all(input.as_bytes()).await;
            let _ = si.shutdown().await;
        });
    }
    let pid = child.id();
    // Windows: the whole process tree lives in a job object, so a timeout or cancel kills all of it and nothing is left behind
    #[cfg(windows)]
    let job = super::winjob::Job::new(std::env::var("FH_MEMORY_LIMIT_MB").ok().and_then(|v| v.parse().ok())).and_then(|j| { pid.map(|p| j.assign(p)); Some(j) });
    let out = tokio::spawn(read_capped(child.stdout.take().unwrap()));
    let err = tokio::spawn(read_capped(child.stderr.take().unwrap()));
    let never = CancellationToken::new();
    let cancel = o.cancel.unwrap_or(never);
    let (mut timed_out, mut aborted) = (false, false);
    let timeout = o.timeout.unwrap_or(Duration::from_secs(24 * 3600));
    let status = tokio::select! {
        s = child.wait() => s.ok(),
        _ = tokio::time::sleep(timeout) => { timed_out = true; #[cfg(windows)] if let Some(j) = &job { j.terminate(); } if let Some(p) = pid { kill_tree(p); } child.wait().await.ok() }
        _ = cancel.cancelled() => { aborted = true; #[cfg(windows)] if let Some(j) = &job { j.terminate(); } if let Some(p) = pid { kill_tree(p); } child.wait().await.ok() }
    };
    // descendants may keep the pipes open after the shell exits; do not wait on them past a short grace period
    let stdout = tokio::time::timeout(Duration::from_secs(2), out).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    let stderr = tokio::time::timeout(Duration::from_secs(2), err).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    RunResult { code: status.and_then(|s| s.code()), stdout, stderr, timed_out, aborted, ms: t0.elapsed().as_millis() as u64 }
}

/// Convenience: run with a timeout and no cancellation.
pub async fn run_simple(cmd: &str, cwd: &Path, timeout_ms: u64) -> RunResult {
    run(cmd, cwd, RunOpts { timeout: Some(Duration::from_millis(timeout_ms)), ..Default::default() }).await
}
