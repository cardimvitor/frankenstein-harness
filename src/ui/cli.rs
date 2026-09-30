use crate::config::{auth_headers, load_config, process_env, Config, Env};
use crate::engine::{Engine, HeadlessIo, TaskOptions, TaskResult};
use crate::eval::runner::{run_eval, EvalOptions};
use crate::session::checkpoint::Checkpoints;
use crate::skills::store::SkillStore;
use crate::types::Mode;
use crate::ui::server::{start_web_server, ServeOptions};
use crate::ui::term::*;
use crate::util::proc::run_simple;
use crate::util::sandbox::sandbox_for;
use crate::validate::vllm::{validate_vllm, ValidateOptions};
use serde_json::Value;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const HELP: &str = "Frankenstein Harness (fh) — coding agent for Qwen on vLLM

Usage:
  fh                          full-screen terminal UI in the current directory (use --plain for a line chat)
  fh tui                      the same, explicitly
  fh run \"<task>\" [options]   run one task
  fh serve [--port N]         local web UI (127.0.0.1 only)
  fh doctor                   check endpoint, model, auth, metrics, sandbox
  fh validate-vllm [options]  measure MTP, prefix cache, tool calls, long context, concurrency
  fh eval --tasks <dir> [options]   run the eval corpus (--runner fh|qwen|both)
  fh undo                     restore the working tree to the last checkpoint
  fh activity                 recent skill activity
  fh history [skill]          version history (hash, reason, diff) of learned skills
  fh stats                    runtime statistics: verdicts, rounds, reviewer quality, tokens per task
  fh auth set|clear|status    store the API key in the OS keychain (Linux secret-tool, macOS Keychain)

Options for run/chat:
  --auto            autonomous: no plan approval or questions, up to 5 verification rounds
  --mode <m>        approval mode: plan | ask | auto-edit (default) | yolo
  --yes             never ask (approve plan and tool calls)
  --plan-only       show the plan and stop
  --commit          git commit if verification passes
  --keep            keep changes even when verification fails
  --sandbox         confine shell commands (bwrap on Linux, Seatbelt on macOS)
  --json            print the machine-readable result
  --cwd <dir>       workspace (default: current directory)
  --thinking        show model reasoning (plain mode; in the TUI press Ctrl+T)
  --plain           line-based chat instead of the full-screen UI

Environment: FH_ENDPOINT, FH_MODEL, FH_API_KEY (or the variable named by FH_API_KEY_ENV), FH_AUTH_SCHEME, FH_METRICS_URL, FH_HOME";

pub struct Args {
    pub cmd: String,
    pub positional: Vec<String>,
    pub flags: HashMap<String, String>,
}

impl Args {
    pub fn has(&self, k: &str) -> bool {
        self.flags.contains_key(k)
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.flags.get(k).map(|s| s.as_str()).filter(|s| *s != "true")
    }
}

pub fn parse_args(argv: &[String]) -> Args {
    const VALUED: [&str; 12] = ["mode", "cwd", "port", "tasks", "runner", "out", "limit", "repeat", "max-context", "concurrency", "trials", "qwen-cmd"];
    let mut flags = HashMap::new();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if let Some(k) = a.strip_prefix("--") {
            if let Some((k, v)) = k.split_once('=') {
                flags.insert(k.to_string(), v.to_string());
            } else if VALUED.contains(&k) && i + 1 < argv.len() {
                i += 1;
                flags.insert(k.to_string(), argv[i].clone());
            } else {
                flags.insert(k.to_string(), "true".to_string());
            }
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    let cmd = if positional.is_empty() { String::new() } else { positional.remove(0) };
    Args { cmd, positional, flags }
}

fn print_result(r: &TaskResult, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(&r.to_json()).unwrap());
        return;
    }
    let text = if r.final_text.is_empty() { r.reason.clone() } else { r.final_text.clone() };
    println!("\n{}", match r.verdict.as_str() { "pass" => green(&text), "unverified" => yellow(&text), _ => red(&text) });
    if let Some(p) = &r.rejected_patch {
        println!("{}", dim(&format!("rejected patch saved to {p}")));
    }
    println!("{}", dim(&format!("{:.1}s · {} round(s) · {} tokens out · {} requests{}", r.timings.total_ms as f64 / 1000.0, r.rounds, r.llm.completion_tokens, r.llm.requests, if r.llm.repaired > 0 { format!(" · {} repaired calls", r.llm.repaired) } else { String::new() })));
}

async fn doctor(cfg: &Config, env: &Env) {
    let ok = |m: &str| println!("{} {m}", green("✓"));
    let bad = |m: &str| println!("{} {m}", red("✗"));
    println!("endpoint {}  model {}  auth {}{}", cfg.endpoint, cfg.model, cfg.auth_scheme, if cfg.auth_scheme != "none" { format!(" ({} {})", cfg.api_key_env, if env.get(&cfg.api_key_env).map(|s| !s.is_empty()).unwrap_or(false) { "set" } else { "NOT set" }) } else { String::new() });
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{}/models", cfg.endpoint)).timeout(Duration::from_secs(8));
    for (k, v) in auth_headers(cfg, env) {
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(r) if r.status().as_u16() == 401 || r.status().as_u16() == 403 => bad(&format!("/models rejected credentials (HTTP {})", r.status().as_u16())),
        Ok(r) if !r.status().is_success() => bad(&format!("/models HTTP {}", r.status().as_u16())),
        Ok(r) => {
            let j: Value = r.json().await.unwrap_or(Value::Null);
            let ids: Vec<String> = j["data"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            ok(&format!("models: {}", if ids.is_empty() { "(none)".to_string() } else { ids.join(", ") }));
            if !ids.contains(&cfg.model) {
                bad(&format!("configured model \"{}\" is not served; set FH_MODEL to one of the above", cfg.model));
            }
            if let Some(ml) = j["data"].as_array().and_then(|a| a.iter().find(|m| m["id"] == cfg.model.as_str())).and_then(|m| m["max_model_len"].as_u64()) {
                ok(&format!("max_model_len {ml}"));
            }
        }
        Err(e) => bad(&format!("cannot reach {}: {e}", cfg.endpoint)),
    }
    let mut req = client.get(&cfg.metrics_url).timeout(Duration::from_secs(4));
    for (k, v) in auth_headers(cfg, env) {
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => ok(&format!("metrics at {}", cfg.metrics_url)),
        Ok(r) => bad(&format!("metrics HTTP {} (governor and MTP/prefix-cache measurements need /metrics)", r.status().as_u16())),
        Err(_) => bad(&format!("metrics unreachable at {}", cfg.metrics_url)),
    }
    let (_, backend) = sandbox_for(&std::env::current_dir().unwrap_or_default(), true);
    if backend == "none" { bad("shell sandbox: none") } else { ok(&format!("shell sandbox: {backend}")) }
    ok(&format!("{} {}", std::env::consts::OS, std::env::consts::ARCH));
}

pub async fn main(argv: Vec<String>) -> i32 {
    let args = parse_args(&argv);
    if args.has("help") || args.cmd == "help" {
        println!("{HELP}");
        return 0;
    }
    let cwd = PathBuf::from(args.get("cwd").map(|s| s.to_string()).unwrap_or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| ".".into())));
    if !cwd.exists() {
        eprintln!("no such directory: {}", cwd.display());
        return 2;
    }
    let mut env = process_env();
    let cfg = match load_config(&cwd, &env) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // API key: env var first, then the OS keychain (unless configured otherwise)
    if cfg.api_key_store != "env" && cfg.auth_scheme != "none" && env.get(&cfg.api_key_env).map(|s| s.is_empty()).unwrap_or(true) {
        if let Some(k) = crate::secrets::get(&cfg.api_key_env) {
            env.insert(cfg.api_key_env.clone(), k);
        }
    }
    let approval = match args.get("mode").map(|m| (m, Mode::parse(m))) {
        None => Mode::AutoEdit,
        Some((_, Some(m))) => m,
        Some((m, None)) => {
            eprintln!("invalid --mode {m}");
            return 2;
        }
    };

    match args.cmd.as_str() {
        "doctor" => {
            doctor(&cfg, &env).await;
            return 0;
        }
        "activity" => {
            match SkillStore::open(&env) {
                Ok(s) => {
                    for a in s.activity(30) {
                        println!("{}  {:<11} {} — {}", a.ts, a.kind, a.skill, a.reason);
                    }
                }
                Err(e) => eprintln!("cannot open skill store: {e}"),
            }
            return 0;
        }
        "undo" => {
            let cp = Checkpoints::new(cwd.clone());
            let last = run_simple("git for-each-ref --sort=-refname --format=%(objectname) refs/fh/checkpoints --count=1", &cwd, 10_000).await.stdout.trim().to_string();
            if last.is_empty() {
                eprintln!("no checkpoint found");
                return 1;
            }
            let files = cp.restore(&last).await;
            println!("restored {} file(s) to checkpoint {}", files.len(), &last[..8.min(last.len())]);
            return 0;
        }
        "stats" => {
            match SkillStore::open(&env) {
                Ok(s) => {
                    let r = s.run_stats();
                    if r.tasks == 0 {
                        println!("no tasks recorded yet");
                    } else {
                        println!("tasks {}: pass {} · fail {} · unverified {} · avg rounds {:.2} · avg tokens/task {:.0}", r.tasks, r.pass, r.fail, r.unverified, r.avg_rounds, r.avg_tokens);
                        println!("reviewer findings raised {} · dropped as uncheckable {} ({:.0}%) · blockers {}", r.reviewer_raised, r.reviewer_dropped, if r.reviewer_raised > 0 { 100.0 * r.reviewer_dropped as f64 / r.reviewer_raised as f64 } else { 0.0 }, r.reviewer_blockers);
                        println!("{}", dim("A high dropped share means the reviewer cites lines it cannot support; verifier false positives are measured against an oracle by `fh eval`."));
                    }
                }
                Err(e) => eprintln!("cannot open skill store: {e}"),
            }
            return 0;
        }
        "history" => {
            match SkillStore::open(&env) {
                Ok(s) => {
                    let rows = s.history(args.positional.first().map(|s| s.as_str()).unwrap_or(""));
                    if rows.is_empty() {
                        println!("no learned skills yet");
                    }
                    for (name, ver, hash, reason, diff) in rows {
                        println!("{} v{ver}  {hash}  {reason}", bold(&name));
                        for l in diff.lines().take(12) {
                            println!("    {}", if l.starts_with('+') { green(l) } else { red(l) });
                        }
                    }
                }
                Err(e) => eprintln!("cannot open skill store: {e}"),
            }
            return 0;
        }
        "auth" => {
            let acct = cfg.api_key_env.clone();
            match args.positional.first().map(|s| s.as_str()) {
                Some("set") => {
                    let k = tokio::task::spawn_blocking(|| read_secret_blocking("API key (input hidden): ")).await.unwrap_or_default();
                    if k.is_empty() {
                        eprintln!("empty key, nothing stored");
                        return 2;
                    }
                    match crate::secrets::set(&acct, &k) {
                        Ok(()) => println!("stored in the OS keychain as {acct}"),
                        Err(e) => {
                            eprintln!("could not store the key: {e}");
                            return 1;
                        }
                    }
                }
                Some("clear") => match crate::secrets::clear(&acct) {
                    Ok(()) => println!("removed {acct} from the OS keychain"),
                    Err(e) => {
                        eprintln!("{e}");
                        return 1;
                    }
                },
                _ => println!("{acct}: env {} · keychain {}", if env.get(&acct).map(|s| !s.is_empty()).unwrap_or(false) { "set" } else { "not set" }, if crate::secrets::get(&acct).is_some() { "set" } else { "not set" }),
            }
            return 0;
        }
        "validate-vllm" => {
            return validate_vllm(&cfg, &env, ValidateOptions { out: PathBuf::from(args.get("out").unwrap_or("reports")), quick: args.has("quick"), max_context: args.get("max-context").and_then(|s| s.parse().ok()), trials: args.get("trials").and_then(|s| s.parse().ok()), concurrency: args.get("concurrency").and_then(|s| s.parse().ok()) }).await;
        }
        "eval" => {
            return run_eval(&cfg, &env, EvalOptions { tasks: PathBuf::from(args.get("tasks").unwrap_or("eval/tasks")), runner: args.get("runner").unwrap_or("fh").to_string(), out: PathBuf::from(args.get("out").unwrap_or("reports")), limit: args.get("limit").and_then(|s| s.parse().ok()), repeat: args.get("repeat").and_then(|s| s.parse().ok()).unwrap_or(1), qwen_cmd: args.get("qwen-cmd").unwrap_or("qwen").to_string() }).await;
        }
        "summarize" => {
            // hidden: used by scripts/vps-validate.sh
            let dir = PathBuf::from(args.positional.first().cloned().unwrap_or_else(|| ".".into()));
            println!("{}", crate::validate::summary::summarize_run(&dir, args.positional.get(1).map(|s| s.as_str()).unwrap_or("")));
            return 0;
        }
        "mock-server" => {
            // hidden: scripted mock vLLM for dry-running scripts/vps-validate.sh without a model
            let mut m = crate::testkit::start(args.get("port").and_then(|s| s.parse().ok()).unwrap_or(18000), None).await;
            crate::testkit::install_smart(&m);
            println!("mock vLLM on {}", m.url);
            let _ = tokio::signal::ctrl_c().await;
            m.stop();
            return 0;
        }
        "serve" => {
            let log: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(|m| println!("{m}"));
            let srv = match start_web_server(cfg.clone(), env.clone(), cwd.clone(), ServeOptions { port: args.get("port").and_then(|s| s.parse().ok()).unwrap_or(7878), store: None, log: Some(log) }).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cannot start web UI: {e}");
                    return 1;
                }
            };
            println!("Frankenstein Harness UI: {}\nOne-time access code: {}  (enter it in the page; it works once)\nBound to 127.0.0.1 only. Press Ctrl+C to stop.", srv.url, bold(&srv.code()));
            let _ = tokio::signal::ctrl_c().await;
            return 0;
        }
        _ => {}
    }

    let store = match SkillStore::open(&env) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open skill store: {e}");
            return 1;
        }
    };
    let io: Arc<dyn crate::engine::Io> = if args.has("yes") { Arc::new(TerminalIo::new(args.has("thinking"), true, env.clone())) } else { Arc::new(TerminalIo::new(args.has("thinking"), false, env.clone())) };
    let engine = Engine::new(cfg.clone(), env.clone(), io, cwd.clone(), store);
    let cancel = CancellationToken::new();
    {
        let c = cancel.clone();
        tokio::spawn(async move {
            loop {
                if tokio::signal::ctrl_c().await.is_err() {
                    break;
                }
                if c.is_cancelled() {
                    std::process::exit(130);
                }
                println!("{}", yellow("\ncancelling… (Ctrl+C again to force)"));
                c.cancel();
            }
        });
    }
    let opts = |cancel: Option<CancellationToken>| TaskOptions { auto: args.has("auto"), approval, commit: args.has("commit"), plan_only: args.has("plan-only"), keep_on_fail: args.has("keep"), sandbox: args.has("sandbox"), no_mine: false, cancel };

    match args.cmd.as_str() {
        "run" => {
            let task = args.positional.join(" ");
            let task = task.trim();
            if task.is_empty() {
                eprintln!("usage: fh run \"<task>\"");
                return 2;
            }
            let r = engine.run_task(task, opts(Some(cancel))).await;
            print_result(&r, args.has("json"));
            engine.drain(20_000).await;
            match r.verdict.as_str() {
                "pass" | "planned" => 0,
                "unverified" => 3,
                _ => 1,
            }
        }
        "tui" | "" if args.cmd == "tui" || (std::io::stdout().is_terminal() && std::io::stdin().is_terminal() && !args.has("plain")) => {
            drop(engine);
            let store = match SkillStore::open(&env) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cannot open skill store: {e}");
                    return 1;
                }
            };
            crate::tui::run::run_tui(cfg.clone(), env.clone(), cwd.clone(), store, crate::tui::run::TuiOptions { approval, auto: args.has("auto"), sandbox: args.has("sandbox"), commit: args.has("commit") }).await
        }
        "" | "chat" => {
            println!("{}", bold("Frankenstein Harness") + &dim(&format!("  {} @ {}  ·  {} · {:?}  ·  /exit to quit", cfg.model, cfg.endpoint, if args.has("auto") { "auto" } else { "guided" }, approval)));
            loop {
                let line = ask_line(&cyan("› ")).await;
                if line.is_empty() {
                    continue;
                }
                if line == "/exit" || line == "/quit" {
                    break;
                }
                let r = engine.run_task(&line, opts(None)).await;
                print_result(&r, false);
                engine.drain(20_000).await;
            }
            0
        }
        other => {
            eprintln!("unknown command: {other}\n\n{HELP}");
            2
        }
    }
}

#[allow(dead_code)]
fn _unused(_: HeadlessIo) {}
