use crate::config::{redact, Env};
use crate::engine::{Io, NoticeKind, PlanDecision};
use crate::skills::reuse::ReuseOffer;
use async_trait::async_trait;
use serde_json::Value;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

fn color_on() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}
fn paint(code: &str, s: &str) -> String {
    if color_on() { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
}
pub fn dim(s: &str) -> String { paint("2", s) }
pub fn bold(s: &str) -> String { paint("1", s) }
pub fn green(s: &str) -> String { paint("32", s) }
pub fn red(s: &str) -> String { paint("31", s) }
pub fn yellow(s: &str) -> String { paint("33", s) }
pub fn cyan(s: &str) -> String { paint("36", s) }

#[cfg(unix)]
struct RawGuard(libc::termios);
#[cfg(unix)]
impl RawGuard {
    fn new(echo_off_only: bool) -> Option<RawGuard> {
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return None;
            }
            let saved = t;
            if echo_off_only {
                t.c_lflag &= !libc::ECHO;
            } else {
                t.c_lflag &= !(libc::ICANON | libc::ECHO);
                t.c_cc[libc::VMIN] = 1;
                t.c_cc[libc::VTIME] = 0;
            }
            if libc::tcsetattr(0, libc::TCSANOW, &t) != 0 {
                return None;
            }
            Some(RawGuard(saved))
        }
    }
}
#[cfg(unix)]
impl Drop for RawGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.0);
        }
    }
}

/// Read a single keypress (no Enter needed). Returns None when stdin is not a terminal or on unsupported platforms.
pub fn read_key_blocking() -> Option<char> {
    #[cfg(unix)]
    {
        if !std::io::stdin().is_terminal() {
            return None;
        }
        let _g = RawGuard::new(false)?;
        let mut b = [0u8; 1];
        let n = unsafe { libc::read(0, b.as_mut_ptr() as *mut libc::c_void, 1) };
        if n == 1 {
            return Some(match b[0] { b'\r' | b'\n' => '\n', 3 => '\u{3}', c => c as char });
        }
        None
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Prompt for a single key; falls back to a line prompt when there is no terminal. Enter is returned as '\n'.
pub async fn ask_key(prompt: &str) -> char {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let k = tokio::task::spawn_blocking(read_key_blocking).await.ok().flatten();
    match k {
        Some('\u{3}') => std::process::exit(130),
        Some(c) => {
            println!("{}", if c == '\n' { String::new() } else { c.to_string() });
            c.to_ascii_lowercase()
        }
        None => ask_line("").await.chars().next().map(|c| c.to_ascii_lowercase()).unwrap_or('\n'),
    }
}

/// Read a secret line without echo (unix terminals); plain line otherwise.
pub fn read_secret_blocking(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    #[cfg(unix)]
    let _g = if std::io::stdin().is_terminal() { RawGuard::new(true) } else { None };
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    println!();
    s.trim().to_string()
}

/// Read one line from stdin without blocking the runtime.
pub async fn ask_line(prompt: &str) -> String {
    let p = prompt.to_string();
    tokio::task::spawn_blocking(move || {
        print!("{p}");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        s.trim().to_string()
    })
    .await
    .unwrap_or_default()
}

fn short(v: &Value, env: &Env) -> String {
    redact(&v.to_string(), env).chars().take(110).collect()
}

pub struct TerminalIo {
    pub show_reasoning: bool,
    pub assume_yes: bool,
    pub env: Env,
    in_text: AtomicBool,
}

impl TerminalIo {
    pub fn new(show_reasoning: bool, assume_yes: bool, env: Env) -> Self {
        TerminalIo { show_reasoning, assume_yes, env, in_text: AtomicBool::new(false) }
    }
    fn end_text(&self) {
        if self.in_text.swap(false, Ordering::SeqCst) {
            println!();
        }
    }
}

#[async_trait]
impl Io for TerminalIo {
    fn notice(&self, kind: NoticeKind, msg: &str) {
        self.end_text();
        let tag = match kind {
            NoticeKind::Phase => cyan("▸"),
            NoticeKind::Skill => yellow("✦"),
            NoticeKind::Verify => green("✓"),
            NoticeKind::Warn => red("!"),
            NoticeKind::Info => dim("·"),
        };
        println!("{tag} {}", if kind == NoticeKind::Info { dim(msg) } else { msg.to_string() });
    }
    fn progress(&self, d: &str) {
        print!("{}", dim(d));
        let _ = std::io::stdout().flush();
        self.in_text.store(true, Ordering::SeqCst);
    }
    fn reasoning(&self, d: &str) {
        if self.show_reasoning {
            self.progress(d);
        }
    }
    fn tool_start(&self, name: &str, args: &Value) {
        self.end_text();
        print!("{}", dim(&format!("  {name} {}", short(args, &self.env))));
        let _ = std::io::stdout().flush();
    }
    fn tool_end(&self, _n: &str, ok: bool, _o: &str, ms: u64) {
        println!(" {} {}", if ok { green("ok") } else { red("fail") }, dim(&format!("{ms}ms")));
    }
    async fn ask_questions(&self, qs: Vec<String>) -> Vec<String> {
        self.end_text();
        println!("{}", bold("A few questions before I start (Enter to let me decide):"));
        let mut out = Vec::new();
        for (i, q) in qs.iter().enumerate() {
            let a = ask_line(&format!("{}. {q}\n   > ", i + 1)).await;
            out.push(if a.is_empty() { "(use your best judgement)".to_string() } else { a });
        }
        out
    }
    async fn approve_plan(&self, plan: &str, trivial: bool) -> PlanDecision {
        self.end_text();
        println!("{}\n{plan}\n", bold(if trivial { "Plan (small change):" } else { "Plan:" }));
        if self.assume_yes {
            return PlanDecision { ok: true, feedback: None };
        }
        let k = ask_key(if trivial { "Enter = go, n = cancel: " } else { "Enter = go, n = cancel, e = give feedback: " }).await;
        match Some(k) {
            Some('n') => PlanDecision { ok: false, feedback: None },
            Some('e') => {
                let fb = ask_line("Feedback: ").await;
                PlanDecision { ok: false, feedback: if fb.is_empty() { None } else { Some(fb) } }
            }
            _ => PlanDecision { ok: true, feedback: None },
        }
    }
    async fn confirm(&self, tool: &str, args: &Value) -> bool {
        self.end_text();
        if self.assume_yes {
            return true;
        }
        println!("{}", yellow(&format!("\n{tool} wants to run: {}", short(args, &self.env))));
        ask_key("Allow? y = once, any other key = deny: ").await == 'y'
    }
    async fn offer_reuse(&self, offers: &[ReuseOffer]) -> Vec<(String, Vec<String>)> {
        self.end_text();
        if self.assume_yes {
            return vec![];
        }
        let mut picks = Vec::new();
        for o in offers {
            println!("{}", bold(&format!("\n\"{}\" has {} skill(s) for a similar stack. Reuse them here?", o.from_label, o.skills.len())));
            for (i, s) in o.skills.iter().enumerate() {
                println!("  {}. {} — {}", i + 1, s.name, s.summary);
            }
            let a = ask_line("  a = reuse all, numbers (e.g. 1,3) = pick, Enter = no: ").await.to_lowercase();
            if a == "a" {
                picks.push((o.from_project.clone(), o.skills.iter().map(|s| s.id.clone()).collect()));
            } else if !a.is_empty() {
                let ids: Vec<String> = a.split(|c: char| c == ',' || c == ' ').filter_map(|n| n.parse::<usize>().ok()).filter_map(|n| o.skills.get(n.wrapping_sub(1)).map(|s| s.id.clone())).collect();
                picks.push((o.from_project.clone(), ids));
            }
        }
        picks
    }
}
