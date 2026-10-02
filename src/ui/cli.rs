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
  fh --version                print the version
  fh delegate \"<task>\" [--no-harness]  Big Frank (27B) plans, a worker model (Small Frank, 9B) (FH_WORKER_ENDPOINT/FH_WORKER_MODEL) writes once, the main model verifies (full fh loop, or one no-tools request with --no-harness)
  fh direct \"<task>\"        the model alone, no harness (one request, edits applied as written): the baseline
  fh doctor                   check endpoint, model, auth, metrics, sandbox
  fh validate-vllm [options]  measure MTP, prefix cache, tool calls, long context, concurrency
  fh eval --tasks <dir> [options]   run the eval corpus (--runner fh|fh-single|qwen|both|all|orch)
  fh undo                     restore the working tree to the last checkpoint
  fh keep                     apply the last rejected patch anyway (recorded as a verifier false positive)
  fh search <words> [--all]   search past task sessions (this repository, or every one with --all)
  fh sessions                 list task sessions of this repository (interrupted ones can be resumed)
  fh resume [id]              continue an interrupted session with its stored plan and checkpoint
  fh record <id> --prompt \"..\" --oracle \"cmd\" [--base rev] [--solution rev] [--out eval/tasks]
                              snapshot your own work as an eval task (oracle must fail on base, pass with the solution)
  fh mcp list|login <server>|logout <server>   MCP servers and OAuth sign-in for remote ones
  fh trust [--revoke|--list]  trust this repository: project hooks (.fh/hooks.json) and MCP servers (.fh/mcp.json) run only then
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
  --no-memory       still create and improve skills, but never put learned skills in the prompt (an ablation of memory)
  --consolidate     with maxConcurrency > 1: parallel scouts/workers, then one agent consolidates all findings and solutions

Environment: FH_ENDPOINT, FH_MODEL, FH_API_KEY (or the variable named by FH_API_KEY_ENV), FH_AUTH_SCHEME, FH_METRICS_URL, FH_CONTEXT_WINDOW, FH_MAX_CONCURRENCY, FH_MEMORY=off, FH_CONSOLIDATE=1, FH_WORKER_ENDPOINT, FH_WORKER_MODEL, FH_WORKER_CONTEXT_WINDOW, FH_SHARED_STORE=1, FH_HOME";

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
    const VALUED: &[&str] = &["mode", "cwd", "port", "tasks", "runner", "out", "limit", "repeat", "max-context", "concurrency", "trials", "qwen-cmd", "prompt", "oracle", "base", "solution", "setup"];
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
    let client = crate::http::client(cfg);
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
    #[cfg(target_os = "linux")]
    {
        let abi = crate::util::landlock::abi();
        if abi >= 1 { ok(&format!("Landlock ABI {abi} (native sandbox{})", if abi >= 4 { ", TCP confinement available" } else { ", no TCP confinement below ABI 4" })) } else { bad("Landlock unavailable on this kernel (bubblewrap is the fallback)") }
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let trusted = crate::trust::is_trusted(env, &cwd);
    ok(&format!("workspace trust: {}", if trusted { "trusted (project hooks, MCP servers and LSP commands load)" } else { "not trusted (`fh trust` enables project hooks, MCP servers, LSP commands)" }));
    let hooks = crate::hooks::Hooks::load(&cwd, env);
    ok(&format!("hooks: {}{}", if hooks.is_empty() { "none active".to_string() } else { "active".to_string() }, if hooks.skipped_untrusted { " (project hooks skipped: untrusted)" } else { "" }));
    let lsp: Vec<String> = crate::verify::lsp::load_specs(&cwd, env).into_iter().map(|s| s.name).collect();
    ok(&format!("language servers: {}", if lsp.is_empty() { "none found for this repository (build/test output decides)".to_string() } else { lsp.join(", ") }));
    let (mcp, notes) = crate::mcp::load_tools(&cwd, env).await;
    ok(&format!("MCP: {} tool(s){}", mcp.len(), if notes.is_empty() { String::new() } else { format!(" — {}", notes.join("; ")) }));
    if !cfg.client_cert.is_empty() { ok(&format!("mTLS client certificate: {}", cfg.client_cert)); }
    if !cfg.auth_token_cmd.is_empty() { ok("token command configured (refreshed after a 401)"); }
    ok(&format!("{} {}", std::env::consts::OS, std::env::consts::ARCH));
}

pub async fn main(argv: Vec<String>) -> i32 {
    let args = parse_args(&argv);
    if args.has("help") || args.cmd == "help" {
        println!("{HELP}");
        return 0;
    }
    if args.has("version") || args.cmd == "version" || argv.first().map(|a| a == "-V").unwrap_or(false) {
        println!("fh {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let cwd = PathBuf::from(args.get("cwd").map(|s| s.to_string()).unwrap_or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| ".".into())));
    if !cwd.exists() {
        eprintln!("no such directory: {}", cwd.display());
        return 2;
    }
    let mut env = process_env();
    let mut cfg = match load_config(&cwd, &env) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if args.has("no-memory") {
        cfg.memory = "off".into();
    }
    if args.has("consolidate") {
        cfg.consolidate = true;
    }
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
        "direct" => {
            // the model alone, no harness: the baseline every harness is compared against
            let task = args.positional.join(" ");
            if task.trim().is_empty() {
                eprintln!("usage: fh direct \"<task>\" [--cwd dir] [--commit]");
                return 2;
            }
            let r = crate::direct::run_direct(&cfg, &env, &cwd, task.trim(), args.has("commit")).await;
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
            return match r["verdict"].as_str() {
                Some("applied") => 0,
                Some("failed") => 1,
                _ => 1,
            };
        }
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
        "record" => {
            let Some(id) = args.positional.first().cloned() else {
                eprintln!("usage: fh record <id> --prompt \"..\" --oracle \"cmd\" [--base rev] [--solution rev] [--out dir]");
                return 2;
            };
            let o = crate::eval::record::RecordOptions { id, prompt: args.get("prompt").unwrap_or("").to_string(), oracle: args.get("oracle").unwrap_or("").to_string(), out: PathBuf::from(args.get("out").unwrap_or("eval/tasks")), base: args.get("base").map(|s| s.to_string()), solution: args.get("solution").map(|s| s.to_string()), setup: args.get("setup").map(|s| s.to_string()), force: args.has("force") };
            return match crate::eval::record::record_task(&cwd, o).await {
                Ok(r) => {
                    println!("recorded {} (oracle fails on base: {}{})", r.dir.display(), r.oracle_fails_on_base, r.oracle_passes_with_solution.map(|p| format!(", passes with the solution: {p}")).unwrap_or_default());
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            };
        }
        "mcp" => {
            let (servers, notes) = crate::mcp::configured_servers(&cwd, &env);
            for n in notes {
                println!("{}", yellow(&n));
            }
            let store = crate::mcp::oauth::OAuthStore::new(crate::config::data_dir(&env).join("mcp-oauth.json"));
            let find = |name: Option<&String>| -> Option<(String, crate::mcp::ServerConfig)> { name.and_then(|n| servers.iter().find(|(x, _)| x == n).cloned()) };
            match args.positional.first().map(|s| s.as_str()) {
                Some("login") => {
                    let Some((name, cfg)) = find(args.positional.get(1)) else {
                        eprintln!("usage: fh mcp login <server>  (servers: {})", servers.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "));
                        return 2;
                    };
                    let crate::mcp::ServerConfig::Http { url, headers } = cfg else {
                        eprintln!("{name} is a local (stdio) server; OAuth sign-in applies to remote servers");
                        return 2;
                    };
                    let opener: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(|u| {
                        println!("Open this URL to sign in:\n\n  {u}\n\nWaiting for the browser (5 minutes)…");
                        let _ = if cfg!(target_os = "macos") { std::process::Command::new("open").arg(u).spawn() } else if cfg!(windows) { std::process::Command::new("cmd").args(["/C", "start", "", u]).spawn() } else { std::process::Command::new("xdg-open").arg(u).stderr(std::process::Stdio::null()).stdout(std::process::Stdio::null()).spawn() };
                    });
                    return match crate::mcp::oauth::login(&name, &url, &headers, &store, opener, Duration::from_secs(300)).await {
                        Ok(()) => {
                            println!("{}", green(&format!("signed in to {name}")));
                            0
                        }
                        Err(e) => {
                            eprintln!("sign-in failed: {e}");
                            1
                        }
                    };
                }
                Some("logout") => {
                    let name = args.positional.get(1).cloned().unwrap_or_default();
                    match store.remove(&name) {
                        Ok(true) => println!("signed out of {name}"),
                        Ok(false) => println!("{name}: no stored sign-in"),
                        Err(e) => eprintln!("{e}"),
                    }
                    return 0;
                }
                _ => {
                    if servers.is_empty() {
                        println!("no MCP servers configured (.fh/mcp.json or the user mcp.json)");
                    }
                    for (n, c) in &servers {
                        match c {
                            crate::mcp::ServerConfig::Stdio { command, .. } => println!("{n:<20} stdio  {command}"),
                            crate::mcp::ServerConfig::Http { url, .. } => println!("{n:<20} http   {url}  {}", if store.get(n).is_some() { green("signed in") } else { dim("no stored sign-in (fh mcp login if the server needs it)") }),
                        }
                    }
                    return 0;
                }
            }
        }
        "trust" => {
            if args.has("list") {
                for p in crate::trust::list(&env) {
                    println!("{p}");
                }
            } else if args.has("revoke") {
                match crate::trust::untrust(&env, &cwd) {
                    Ok(()) => println!("no longer trusting {}", cwd.display()),
                    Err(e) => eprintln!("{e}"),
                }
            } else {
                match crate::trust::trust(&env, &cwd) {
                    Ok(()) => println!("trusting {}: its .fh/hooks.json and .fh/mcp.json will now run", cwd.display()),
                    Err(e) => eprintln!("{e}"),
                }
            }
            return 0;
        }
        "search" => {
            let query = args.positional.join(" ");
            if query.trim().is_empty() {
                eprintln!("usage: fh search <words> [--all] [--limit N]");
                return 2;
            }
            let fp = crate::fingerprint::fingerprint(&cwd);
            let hits = crate::session::search::search(&env, if args.has("all") { None } else { Some(fp.project_id.as_str()) }, &query, args.get("limit").and_then(|s| s.parse().ok()).unwrap_or(10));
            if hits.is_empty() {
                println!("no past tasks match \"{query}\"{}", if args.has("all") { "" } else { " in this repository (try --all)" });
            }
            for h in hits {
                println!("{}  {:<11} {}\n    {}", h.id, h.verdict.clone().unwrap_or_else(|| "interrupted".into()), bold(&h.task), dim(&h.snippet));
            }
            return 0;
        }
        "sessions" => {
            let fp = crate::fingerprint::fingerprint(&cwd);
            let rows = crate::session::log::list(&env, &fp.project_id);
            if rows.is_empty() {
                println!("no sessions yet");
            }
            for s in rows.iter().take(20) {
                let state = match &s.verdict {
                    Some(v) => v.clone(),
                    None => yellow("interrupted (fh resume)").to_string(),
                };
                println!("{}  {:<24} {}", s.id, state, s.task.chars().take(70).collect::<String>());
            }
            return 0;
        }
        "keep" => {
            let fp = crate::fingerprint::fingerprint(&cwd);
            let last = crate::session::signals::read_last_task(&cwd);
            let Some(patch) = last.as_ref().and_then(|v| v["rejectedPatch"].as_str()).map(|s| s.to_string()) else {
                eprintln!("no rejected patch to keep");
                return 1;
            };
            let r = run_simple(&format!("git apply --whitespace=nowarn {patch:?}"), &cwd, 30_000).await;
            if r.code != Some(0) {
                eprintln!("could not apply {patch}: {}", r.stderr.trim());
                return 1;
            }
            crate::session::signals::record_signal(&env, &fp.project_id, "kept_rejected");
            crate::session::signals::mark_handled(&cwd);
            println!("applied {patch}");
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
            if let Some(lt) = crate::session::signals::read_last_task(&cwd) {
                if matches!(lt["verdict"].as_str(), Some("pass") | Some("unverified")) && lt["handled"] != true {
                    crate::session::signals::record_signal(&env, &crate::fingerprint::fingerprint(&cwd).project_id, "undid_pass");
                    crate::session::signals::mark_handled(&cwd);
                }
            }
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
                        let sg = crate::session::signals::signal_counts(&env);
                        println!("user signals: undid a passing task {}× (possible verifier false negative) · applied a rejected patch {}× (possible false positive)", sg.undid_pass, sg.kept_rejected);
                        let ts = crate::session::signals::telemetry_summary(&env);
                        if ts.tasks > 0 {
                            println!("telemetry ({} tasks): p50 {:.1}s · p95 {:.1}s{} · {} repaired tool calls", ts.tasks, ts.p50_ms as f64 / 1000.0, ts.p95_ms as f64 / 1000.0, ts.avg_acceptance.map(|a| format!(" · MTP acceptance {:.0}%", a * 100.0)).unwrap_or_default(), ts.repaired_calls);
                        }
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

    let store = {
        let (s, note) = SkillStore::open_resilient(&env);
        if let Some(n) = note {
            eprintln!("warning: {n}");
        }
        s
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
    let opts = |cancel: Option<CancellationToken>| TaskOptions { auto: args.has("auto"), approval, commit: args.has("commit"), plan_only: args.has("plan-only"), keep_on_fail: args.has("keep"), sandbox: args.has("sandbox"), no_mine: false, cancel, resume: None };

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
        "delegate" => {
            // 27B+9B experiment: the main model plans, a worker model writes once, the main model verifies and fixes
            let task = args.positional.join(" ");
            let task = task.trim();
            if task.is_empty() {
                eprintln!("usage: fh delegate \"<task>\"  (needs FH_WORKER_ENDPOINT and FH_WORKER_MODEL)");
                return 2;
            }
            let wcfg = match crate::delegate::worker_config(&cfg, &env) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("{e}");
                    return 2;
                }
            };
            let prep = crate::delegate::prepare(&cfg, &wcfg, &env, &cwd, task).await;
            if let Some(e) = &prep.error {
                let n = prep.planner.requests + prep.worker.requests;
                let r = serde_json::json!({"verdict": "error", "reason": e, "final": "", "changed": [], "rounds": 0, "workers": [],
                    "llm": {"requests": n, "promptTokens": prep.planner.prompt_tokens + prep.worker.prompt_tokens, "completionTokens": prep.planner.completion_tokens + prep.worker.completion_tokens, "cachedTokens": 0, "toolCalls": 0},
                    "delegate": crate::delegate::report_json(&prep)});
                println!("{}", serde_json::to_string_pretty(&r).unwrap());
                return 1;
            }
            if args.has("no-harness") {
                // pure 27B+9B: the verifier is one no-tools request, applied as written
                let t0 = std::time::Instant::now();
                let v = crate::delegate::verify_pure(&cfg, &env, &cwd, task, &prep).await;
                let mut changed = prep.changed.clone();
                for c in &v.changed {
                    if !changed.contains(c) {
                        changed.push(c.clone());
                    }
                }
                if args.has("commit") && !changed.is_empty() {
                    crate::util::proc::run_simple("git add -A && git -c user.name=fh -c user.email=fh@local commit -qm 'delegate: 27B+9B, no harness'", &cwd, 30_000).await;
                }
                let n = prep.planner.requests + prep.worker.requests + v.stats.requests;
                let verdict = if v.error.is_some() { "error" } else if changed.is_empty() { "failed" } else { "applied" };
                let r = serde_json::json!({"verdict": verdict, "reason": v.error.clone().unwrap_or_default(), "final": v.reply.chars().take(3000).collect::<String>(), "changed": changed, "rounds": 0, "workers": [],
                    "llm": {"requests": n, "promptTokens": prep.planner.prompt_tokens + prep.worker.prompt_tokens + v.stats.prompt_tokens, "completionTokens": prep.planner.completion_tokens + prep.worker.completion_tokens + v.stats.completion_tokens, "cachedTokens": prep.planner.cached_tokens + prep.worker.cached_tokens + v.stats.cached_tokens, "toolCalls": 0},
                    "timings": {"totalMs": t0.elapsed().as_millis() as u64},
                    "delegate": crate::delegate::report_json(&prep), "verifier": {"edits": v.edits, "changed": v.changed, "failed": v.failed.iter().map(|(f, w)| serde_json::json!({"path": f, "why": w})).collect::<Vec<_>>(), "stats": crate::delegate::stats_json(&v.stats)}});
                println!("{}", serde_json::to_string_pretty(&r).unwrap());
                return if verdict == "applied" { 0 } else { 1 };
            }
            let r = engine.run_task(&crate::delegate::verification_task(task, &prep), opts(Some(cancel))).await;
            engine.drain(20_000).await;
            let mut j = r.to_json();
            for k in ["requests", "promptTokens", "completionTokens", "cachedTokens"] {
                let (a, b, c) = match k {
                    "requests" => (prep.planner.requests, prep.worker.requests, 0),
                    "promptTokens" => (prep.planner.prompt_tokens, prep.worker.prompt_tokens, 0),
                    "completionTokens" => (prep.planner.completion_tokens, prep.worker.completion_tokens, 0),
                    _ => (prep.planner.cached_tokens, prep.worker.cached_tokens, 0),
                };
                let cur = j["llm"][k].as_u64().unwrap_or(0);
                j["llm"][k] = serde_json::json!(cur + a + b + c);
            }
            j["delegate"] = crate::delegate::report_json(&prep);
            if args.has("json") {
                println!("{}", serde_json::to_string_pretty(&j).unwrap());
            } else {
                print_result(&r, false);
            }
            return match r.verdict.as_str() {
                "pass" | "planned" => 0,
                "unverified" => 3,
                _ => 1,
            };
        }
        "resume" => {
            let (task, state) = match crate::engine::load_resume(&env, &cwd, args.positional.first().map(|s| s.as_str())) {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            let mut o = opts(Some(cancel));
            o.resume = Some(state);
            let r = engine.run_task(&task, o).await;
            print_result(&r, args.has("json"));
            engine.drain(20_000).await;
            if matches!(r.verdict.as_str(), "pass" | "planned") { 0 } else if r.verdict == "unverified" { 3 } else { 1 }
        }
        "tui" | "" if args.cmd == "tui" || (std::io::stdout().is_terminal() && std::io::stdin().is_terminal() && !args.has("plain")) => {
            drop(engine);
            let store = {
                let (s, note) = SkillStore::open_resilient(&env);
                if let Some(n) = note {
                    eprintln!("warning: {n}");
                }
                s
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
