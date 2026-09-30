use crate::config::{auth_headers, Config, Env};
use crate::eval::runner::chrono_stamp;
use crate::llm::client::{ChatOptions, LlmClient};
use crate::llm::metrics::{delta, fetch_metrics, VllmMetrics};
use crate::tools::all_tools;
use crate::types::{Message, Thinking, ToolSpec};
use futures_util::future::join_all;
use serde_json::{json, Value};
use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Info,
    Skipped,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Info => "info",
            Status::Skipped => "skipped",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub name: String,
    pub status: Status,
    pub summary: String,
    pub data: Value,
}

type Out = Result<(Status, String, Value), String>;

#[derive(Clone, Debug, Default)]
pub struct ValidateOptions {
    pub out: PathBuf,
    pub quick: bool,
    pub max_context: Option<usize>,
    pub trials: Option<usize>,
    pub concurrency: Option<usize>,
}

fn pct(x: f64) -> String {
    format!("{:.1}%", 100.0 * x)
}
fn median(a: &[f64]) -> f64 {
    let mut s = a.to_vec();
    s.sort_by(|x, y| x.partial_cmp(y).unwrap());
    if s.is_empty() { 0.0 } else { s[s.len() / 2] }
}
fn p95(a: &[f64]) -> f64 {
    let mut s = a.to_vec();
    s.sort_by(|x, y| x.partial_cmp(y).unwrap());
    if s.is_empty() { 0.0 } else { s[((0.95 * s.len() as f64).ceil() as usize).clamp(1, s.len()) - 1] }
}

/// Deterministic filler of roughly `tokens` tokens; the real count is read back from usage.
pub fn filler(tokens: usize, seed: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut chars = 0usize;
    let mut i = 0usize;
    let states = ["ready", "queued", "done", "held"];
    while (chars as f64) < tokens as f64 * 3.4 {
        let line = format!("Record {seed}-{i}: unit={} status={} note=alpha{} beta{}.", (i * 7919 + seed) % 997, states[(i + seed) % 4], i % 13, (i * 3) % 17);
        chars += line.len() + 1;
        out.push(line);
        i += 1;
    }
    out.join("\n")
}

struct Scenario {
    name: &'static str,
    prompt: &'static str,
    expect: &'static str,
    required: &'static [&'static str],
}

const SCENARIOS: [Scenario; 5] = [
    Scenario { name: "read", prompt: "Read the file src/app.ts using the tool.", expect: "read_file", required: &["path"] },
    Scenario { name: "edit-multiline", prompt: "In lib/util.js replace the two-line snippet \"const a = 1;\\nconst b = \\\"two\\\";\" with \"const a = 10;\\nconst b = \\\"three\\\"; // updated\". Use the edit tool.", expect: "edit", required: &["path", "old_text", "new_text"] },
    Scenario { name: "bash-quotes", prompt: "Run this exact shell command with the bash tool: grep -rn \"TODO(\\\"x\\\")\" src | head -5", expect: "bash", required: &["command"] },
    Scenario { name: "unicode-write", prompt: "Create the new file docs/notes.md containing the text: Olá, mundo — “aspas” and a tab\\there. Use write_file.", expect: "write_file", required: &["path", "content"] },
    Scenario { name: "grep", prompt: "Search the repo for the regex function\\s+\\w+\\( in *.ts files using grep.", expect: "grep", required: &["pattern"] },
];

fn tools() -> Vec<ToolSpec> {
    all_tools().iter().map(|t| t.spec().clone()).collect()
}

async fn raw_chat(cfg: &Config, env: &Env, body: Value) -> (u16, Value) {
    let client = crate::http::client(cfg);
    let mut req = client.post(format!("{}/chat/completions", cfg.endpoint)).timeout(Duration::from_secs(300)).json(&body);
    for (k, v) in auth_headers(cfg, env) {
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(r) => {
            let s = r.status().as_u16();
            (s, r.json::<Value>().await.unwrap_or(Value::Null))
        }
        Err(_) => (0, Value::Null),
    }
}

fn opts(msgs: Vec<Message>, thinking: Thinking, max_tokens: u32) -> ChatOptions {
    ChatOptions { messages: msgs, thinking, max_tokens: Some(max_tokens), ..Default::default() }
}

async fn probe<F: Future<Output = Out>>(results: &mut Vec<Probe>, name: &str, f: F) {
    print!("• {name} … ");
    let t0 = Instant::now();
    let (status, summary, data) = match f.await {
        Ok(v) => v,
        Err(e) => (Status::Fail, e, Value::Null),
    };
    println!("{} ({:.1}s) {}", status.as_str(), t0.elapsed().as_secs_f64(), summary);
    results.push(Probe { name: name.into(), status, summary, data });
}

pub async fn validate_vllm(cfg: &Config, env: &Env, o: ValidateOptions) -> i32 {
    let llm = {
        let mut c = cfg.clone();
        c.retries = 1;
        LlmClient::new(c, env.clone())
    };
    let mut results: Vec<Probe> = Vec::new();
    println!("Validating {} @ {}{}\n", cfg.model, cfg.endpoint, if o.quick { " (quick)" } else { "" });
    let tools = tools();
    let mut max_len: usize = 0;

    // 1. connectivity and auth
    {
        let client = crate::http::client(cfg);
        let get = |url: String, with_auth: bool| {
            let client = client.clone();
            let hdrs = if with_auth { auth_headers(cfg, env) } else { vec![] };
            async move {
                let mut r = client.get(url).timeout(Duration::from_secs(10));
                for (k, v) in hdrs {
                    r = r.header(k, v);
                }
                r.send().await
            }
        };
        let name = "connectivity and auth";
        print!("• {name} … ");
        let out: Out = async {
            let r = get(format!("{}/models", cfg.endpoint), true).await.map_err(|e| format!("cannot reach {}: {e}", cfg.endpoint))?;
            if !r.status().is_success() {
                return Err(format!("/models HTTP {}", r.status().as_u16()));
            }
            let j: Value = r.json().await.map_err(|e| e.to_string())?;
            let ids: Vec<String> = j["data"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            let Some(m) = j["data"].as_array().and_then(|a| a.iter().find(|m| m["id"] == cfg.model.as_str())) else {
                return Err(format!("model \"{}\" not served; available: {}", cfg.model, ids.join(", ")));
            };
            max_len = m["max_model_len"].as_u64().unwrap_or(0) as usize;
            let no_auth = if cfg.auth_scheme == "none" { None } else { Some(get(format!("{}/models", cfg.endpoint), false).await.map(|r| r.status().as_u16()).unwrap_or(0)) };
            let version = match get(format!("{}/version", crate::config::origin_of(&cfg.endpoint)), true).await {
                Ok(r) if r.status().is_success() => r.json::<Value>().await.ok().and_then(|v| v["version"].as_str().map(|s| s.to_string())),
                _ => None,
            };
            let st = if no_auth == Some(200) { Status::Warn } else { Status::Pass };
            Ok((
                st,
                format!(
                    "model ok, max_model_len {}, vLLM {}{}",
                    if max_len > 0 { max_len.to_string() } else { "unknown".into() },
                    version.clone().unwrap_or_else(|| "version unknown".into()),
                    match no_auth { Some(c) => format!(", unauthenticated request -> HTTP {c}{}", if c == 200 { " (endpoint is NOT protected)" } else { "" }), None => String::new() }
                ),
                json!({"maxLen": max_len, "unauthenticatedStatus": no_auth, "vllmVersion": version, "models": ids}),
            ))
        }
        .await;
        let (status, summary, data) = match out {
            Ok(v) => v,
            Err(e) => (Status::Fail, e, Value::Null),
        };
        println!("{} {}", status.as_str(), summary);
        let failed = status == Status::Fail;
        results.push(Probe { name: name.into(), status, summary, data });
        if failed {
            return finish(results, cfg, &o);
        }
    }

    let m0 = fetch_metrics(cfg, env).await;
    probe(&mut results, "metrics endpoint", async {
        let Some(m) = &m0 else { return Ok((Status::Warn, format!("no /metrics at {}: MTP acceptance, prefix-cache and governor measurements will be skipped", cfg.metrics_url), Value::Null)) };
        let (spec, prefix, kv, per) = (m.spec_accepted.is_some(), m.prefix_queries.is_some(), m.kv_usage.is_some(), m.spec_accepted_per_pos.is_some());
        Ok((if spec && prefix && kv { Status::Pass } else { Status::Warn }, format!("spec-decode {spec}, per-position {per}, prefix-cache {prefix}, kv usage {kv}"), json!({"spec": spec, "prefix": prefix, "kv": kv, "perPos": per})))
    })
    .await;

    // 3. streaming speed
    probe(&mut results, "streaming speed (thinking off/on)", async {
        let mut data = json!({});
        for (label, th) in [("off", Thinking::Off), ("high", Thinking::High)] {
            let (mut ttft, mut tps, mut reasoning, mut leak) = (vec![], vec![], false, false);
            for _ in 0..3 {
                let r = llm.chat(opts(vec![Message::user("Write a TypeScript function that debounces another function, with a short explanation.")], th, 400)).await.map_err(|e| e.to_string())?;
                let gen = r.total_ms.saturating_sub(r.ttft_ms).max(1) as f64;
                ttft.push(r.ttft_ms as f64);
                tps.push(r.usage.completion_tokens as f64 * 1000.0 / gen);
                reasoning |= !r.reasoning.is_empty();
                leak |= r.think_leak;
            }
            data[label] = json!({"ttftMs": median(&ttft), "tokPerSec": (median(&tps) * 10.0).round() / 10.0, "reasoningField": reasoning, "thinkLeak": leak});
        }
        let (off, on) = (&data["off"], &data["high"]);
        let bad = if off["reasoningField"] == true { "thinking-off still returned reasoning (enable_thinking not honored)" } else if on["reasoningField"] != true { "thinking-on returned no reasoning_content (check --reasoning-parser qwen3)" } else { "" };
        Ok((if bad.is_empty() { Status::Pass } else { Status::Warn }, format!("off: {} tok/s TTFT {}ms; on: {} tok/s TTFT {}ms{}", off["tokPerSec"], off["ttftMs"], on["tokPerSec"], on["ttftMs"], if bad.is_empty() { String::new() } else { format!("; {bad}") }), data))
    })
    .await;

    // 4. tool-call reliability
    let trials = o.trials.unwrap_or(if o.quick { 10 } else { 40 });
    probe(&mut results, &format!("tool-call reliability ({trials} calls x thinking off/on)"), async {
        let (mut total, mut called, mut right, mut args_ok, mut ok, mut repaired, mut malformed, mut from_content, mut leak) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let per = ((trials as f64 / (SCENARIOS.len() * 2) as f64).round() as usize).max(1);
        let mut per_scenario = json!({});
        for th in [Thinking::Off, Thinking::High] {
            for sc in &SCENARIOS {
                for _ in 0..per {
                    let r = llm
                        .chat(ChatOptions { messages: vec![Message::system("You are a coding agent. Always respond by calling the appropriate tool."), Message::user(sc.prompt)], tools: tools.clone(), thinking: th, max_tokens: Some(900), temperature: Some(0.6), ..Default::default() })
                        .await
                        .map_err(|e| e.to_string())?;
                    total += 1;
                    let e = per_scenario[sc.name].take();
                    let (mut n, mut good) = (e["n"].as_u64().unwrap_or(0) + 1, e["ok"].as_u64().unwrap_or(0));
                    if let Some(c) = r.tool_calls.first() {
                        called += 1;
                        if c.name == sc.expect {
                            right += 1;
                        }
                        match c.parse {
                            crate::types::ParseState::Ok => ok += 1,
                            crate::types::ParseState::Repaired => repaired += 1,
                            crate::types::ParseState::Malformed => malformed += 1,
                        }
                        if c.from_content {
                            from_content += 1;
                        }
                        if r.think_leak {
                            leak += 1;
                        }
                        if c.parse != crate::types::ParseState::Malformed && sc.required.iter().all(|k| c.str_arg(k).map(|s| !s.is_empty()).unwrap_or(false)) {
                            args_ok += 1;
                            good += 1;
                        }
                    }
                    n = n.max(1);
                    per_scenario[sc.name] = json!({"n": n, "ok": good});
                }
            }
        }
        let t = total as f64;
        let (mal_rate, rep_rate) = (malformed as f64 / t, (repaired + malformed) as f64 / t);
        let st = if mal_rate >= 0.01 { Status::Fail } else if rep_rate > 0.05 || leak > 0 { Status::Warn } else { Status::Pass };
        Ok((
            st,
            format!("calls {}, right tool {}, args valid {}, server-clean {}, repaired {}, malformed {} (target <1%), think-leak {leak}", pct(called as f64 / t), pct(right as f64 / t), pct(args_ok as f64 / t), pct(ok as f64 / t), pct(repaired as f64 / t), pct(mal_rate)),
            json!({"total": total, "called": called, "right": right, "argsOk": args_ok, "ok": ok, "repaired": repaired, "malformed": malformed, "fromContent": from_content, "leak": leak, "malformedRate": mal_rate, "repairedRate": rep_rate, "perScenario": per_scenario}),
        ))
    })
    .await;

    // 5. structured output with thinking on
    probe(&mut results, "structured JSON output with thinking on (planner/reviewer path)", async {
        let schema = json!({"type":"object","properties":{"verdict":{"type":"string","enum":["pass","fail"]},"findings":{"type":"array","items":{"type":"string"}}},"required":["verdict","findings"]});
        let n = if o.quick { 3 } else { 6 };
        let (mut direct, mut via_fallback, mut failed) = (0, 0, 0);
        for i in 0..n {
            let r = llm
                .json(ChatOptions { messages: vec![Message::user(format!("Review this change and answer with a verdict and findings: replaced \"<\" with \"<=\" in a loop bound (case {i})."))], thinking: Thinking::High, max_tokens: Some(2500), json_schema: Some(schema.clone()), ..Default::default() })
                .await
                .map_err(|e| e.to_string())?;
            match r.value.as_ref().and_then(|v| v["verdict"].as_str()) {
                Some("pass") | Some("fail") => if r.fallback { via_fallback += 1 } else { direct += 1 },
                _ => failed += 1,
            }
        }
        let st = if failed > 0 { Status::Fail } else if via_fallback > 0 { Status::Warn } else { Status::Pass };
        Ok((st, format!("{direct}/{n} valid JSON directly, {via_fallback}/{n} only via the thinking-off fallback, {failed}/{n} failed{}", if via_fallback > 0 { " (guided decoding and the reasoning parser conflict on this vLLM version; the harness compensates but the planner/reviewer lose thinking)" } else { "" }), json!({"direct": direct, "viaFallback": via_fallback, "failed": failed, "n": n})))
    })
    .await;

    // 6. stream vs non-stream
    probe(&mut results, "streamed vs non-streamed tool call (temperature 0, MTP correctness)", async {
        let (mut same, mut n) = (0, 0);
        for sc in SCENARIOS.iter().take(if o.quick { 2 } else { 5 }) {
            let msgs = vec![Message::system("You are a coding agent. Always respond by calling the appropriate tool."), Message::user(sc.prompt)];
            let o1 = ChatOptions { messages: msgs, tools: tools.clone(), thinking: Thinking::Off, temperature: Some(0.0), max_tokens: Some(600), ..Default::default() };
            let streamed = llm.chat(o1.clone()).await.map_err(|e| e.to_string())?;
            let mut body = llm.build_body(&o1);
            body["stream"] = json!(false);
            body.as_object_mut().unwrap().remove("stream_options");
            let (_, j) = raw_chat(cfg, env, body).await;
            n += 1;
            let tc = &j["choices"][0]["message"]["tool_calls"][0];
            if let (Some(name), Some(first)) = (tc["function"]["name"].as_str(), streamed.tool_calls.first()) {
                if name == first.name {
                    if let Ok(v) = serde_json::from_str::<Value>(tc["function"]["arguments"].as_str().unwrap_or("")) {
                        if v == first.args {
                            same += 1;
                        }
                    }
                }
            }
        }
        Ok((if same == n { Status::Pass } else if same + 1 >= n { Status::Warn } else { Status::Fail }, format!("{same}/{n} identical"), json!({"same": same, "n": n})))
    })
    .await;

    // 7. think leakage
    probe(&mut results, "thinking must not leak into tool arguments or content", async {
        let (mut leaks, mut bad_content, mut n) = (0, 0, 0);
        for i in 0..(if o.quick { 4 } else { 10 }) {
            let r = llm
                .chat(ChatOptions { messages: vec![Message::user(format!("Think carefully about which file could contain the bug in the parser (attempt {i}), then call read_file on src/parser.ts."))], tools: tools.clone(), thinking: Thinking::High, max_tokens: Some(1500), ..Default::default() })
                .await
                .map_err(|e| e.to_string())?;
            n += 1;
            if r.think_leak {
                leaks += 1;
            }
            if r.content.contains("<think>") || r.content.contains("</think>") {
                bad_content += 1;
            }
        }
        Ok((if leaks > 0 || bad_content > 0 { Status::Fail } else { Status::Pass }, format!("{leaks}/{n} tool args with think tags, {bad_content}/{n} content with think tags"), json!({"leaks": leaks, "badContent": bad_content, "n": n})))
    })
    .await;

    // 8. MTP acceptance
    probe(&mut results, "MTP acceptance (structured vs prose)", async {
        if m0.as_ref().and_then(|m| m.spec_accepted).is_none() {
            return Ok((Status::Skipped, "no spec-decode metrics".to_string(), Value::Null));
        }
        let n = if o.quick { 3 } else { 6 };
        let batch = |prompts: Vec<String>, structured: bool| {
            let llm = llm.clone();
            let (cfg, env) = (cfg.clone(), env.clone());
            async move {
                let a = fetch_metrics(&cfg, &env).await;
                for p in prompts {
                    let mut oo = opts(vec![Message::user(p)], Thinking::Off, 350);
                    if structured {
                        oo.json_schema = Some(json!({"type":"object","properties":{"items":{"type":"array","items":{"type":"object","properties":{"id":{"type":"integer"},"name":{"type":"string"},"active":{"type":"boolean"}},"required":["id","name","active"]}}},"required":["items"]}));
                    }
                    let _ = llm.chat(oo).await;
                }
                delta(&a, &fetch_metrics(&cfg, &env).await)
            }
        };
        let prose = batch((0..n).map(|i| format!("Explain in a few paragraphs why idempotency matters in distributed systems (variation {i}).")).collect(), false).await;
        let js = batch((0..n).map(|i| format!("Return 12 sample records as JSON items with id, name and active (variation {i}).")).collect(), true).await;
        let code = batch((0..n).map(|i| format!("Write a complete TypeScript class Stack<T> with push, pop, peek, size and isEmpty, plus doc comments (variation {i}).")).collect(), false).await;
        let f = |d: &crate::llm::metrics::MetricsDelta| d.acceptance_rate.map(pct).unwrap_or_else(|| "n/a".into());
        let pos = code.per_position_acceptance.as_ref().map(|v| v.iter().map(|x| pct(*x)).collect::<Vec<_>>().join(" / ")).unwrap_or_else(|| "n/a".into());
        let j = |d: &crate::llm::metrics::MetricsDelta| json!({"acceptanceRate": d.acceptance_rate, "meanAcceptedPerDraft": d.mean_accepted_per_draft, "perPosition": d.per_position_acceptance});
        Ok((Status::Info, format!("acceptance: prose {}, json {}, code {}; per-position (code): {pos}", f(&prose), f(&js), f(&code)), json!({"prose": j(&prose), "json": j(&js), "code": j(&code)})))
    })
    .await;

    // 9. prefix cache
    probe(&mut results, "prefix cache (stable prefix first vs variable first)", async {
        if m0.as_ref().and_then(|m| m.prefix_queries).is_none() {
            return Ok((Status::Skipped, "no prefix-cache metrics".to_string(), Value::Null));
        }
        let size = if o.quick { 1500 } else { 4000 };
        let prefix = format!("You are a coding agent.\n{}", filler(size, 1));
        let ask = |system: String, user: String| {
            let llm = llm.clone();
            async move { llm.chat(opts(vec![Message::system(system), Message::user(user)], Thinking::Off, 16)).await.map(|r| r.ttft_ms as f64).unwrap_or(0.0) }
        };
        let a0 = fetch_metrics(cfg, env).await;
        let cold = ask(prefix.clone(), "Task 0: say ok.".into()).await;
        let mut warm = vec![];
        for i in 1..=4 {
            warm.push(ask(prefix.clone(), format!("Task {i}: say ok.")).await);
        }
        let stable = delta(&a0, &fetch_metrics(cfg, env).await);
        let b0 = fetch_metrics(cfg, env).await;
        let mut var_first = vec![];
        for i in 0..4 {
            let sys = format!("Session {}-{i}.\n{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0), filler(size, 2));
            var_first.push(ask(sys, "say ok.".into()).await);
        }
        let variable = delta(&b0, &fetch_metrics(cfg, env).await);
        let ok = stable.prefix_hit_rate.unwrap_or(0.0) > variable.prefix_hit_rate.unwrap_or(0.0);
        let f = |d: Option<f64>| d.map(pct).unwrap_or_else(|| "n/a".into());
        Ok((
            if ok { Status::Pass } else { Status::Warn },
            format!("stable prefix: hit {}, TTFT cold {}ms -> warm {}ms; variable-first: hit {}, TTFT {}ms{}", f(stable.prefix_hit_rate), cold as u64, median(&warm) as u64, f(variable.prefix_hit_rate), median(&var_first) as u64, if ok { "" } else { " (prefix caching not visibly effective: is --enable-prefix-caching on?)" }),
            json!({"stableHit": stable.prefix_hit_rate, "variableHit": variable.prefix_hit_rate, "coldMs": cold, "warmMs": warm, "varFirstMs": var_first}),
        ))
    })
    .await;

    // 10. long context
    probe(&mut results, "long-context retrieval and latency", async {
        let limit = o.max_context.unwrap_or(if max_len > 0 { max_len * 8 / 10 } else { 32000 });
        let all: Vec<usize> = if o.quick { vec![4000, 16000] } else { vec![4000, 16000, 32000, 64000, 100000, 128000] };
        let mut rows = vec![];
        for size in all.into_iter().filter(|s| *s <= limit) {
            let secret = format!("KX-{}", rand::random::<u32>() % 90000 + 10000);
            let mut lines: Vec<String> = filler(size, size).split('\n').map(|s| s.to_string()).collect();
            let at = lines.len() * 4 / 10;
            lines.insert(at, format!("The maintenance passphrase is {secret}."));
            let t0 = Instant::now();
            let r = llm.chat(opts(vec![Message::user(format!("{}\n\nWhat is the maintenance passphrase? Answer with the passphrase only.", lines.join("\n")))], Thinking::Off, 30)).await.map_err(|e| e.to_string())?;
            rows.push(json!({"target": size, "promptTokens": r.usage.prompt_tokens, "correct": r.content.contains(&secret), "ttftMs": r.ttft_ms, "totalMs": t0.elapsed().as_millis() as u64}));
        }
        let wrong = rows.iter().filter(|r| r["correct"] != true).count();
        let summary = rows.iter().map(|r| format!("{} tok: {} TTFT {}ms", r["promptTokens"], if r["correct"] == true { "ok" } else { "MISS" }, r["ttftMs"])).collect::<Vec<_>>().join("; ");
        Ok((if wrong > 0 { Status::Warn } else { Status::Pass }, if summary.is_empty() { "no size fits max_model_len".into() } else { summary }, json!({"rows": rows})))
    })
    .await;

    // 11. concurrency and KV pressure
    probe(&mut results, "concurrency and KV pressure (governor calibration)", async {
        let levels: Vec<usize> = match o.concurrency { Some(c) => vec![1, c], None => if o.quick { vec![1, 2, 4] } else { vec![1, 2, 4, 8] } };
        let ctx = if o.quick { 4000 } else { 8000 };
        let (mut table, mut single, mut recommended) = (vec![], 0.0f64, 1usize);
        for k in levels {
            let stop = CancellationToken::new();
            let samples = Arc::new(std::sync::Mutex::new(Vec::<VllmMetrics>::new()));
            let sampler = {
                let (cfg, env, stop, samples) = (cfg.clone(), env.clone(), stop.clone(), samples.clone());
                tokio::spawn(async move {
                    while !stop.is_cancelled() {
                        if let Some(m) = fetch_metrics(&cfg, &env).await {
                            samples.lock().unwrap().push(m);
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                })
            };
            let t0 = Instant::now();
            let lat = join_all((0..k).map(|i| {
                let llm = llm.clone();
                async move {
                    let t = Instant::now();
                    let r = llm.chat(opts(vec![Message::user(format!("{}\n\nSummarize the records above in one sentence.", filler(ctx, 100 + i + k * 10)))], Thinking::Off, 96)).await;
                    (t.elapsed().as_millis() as f64, r.map(|r| r.usage.completion_tokens).unwrap_or(0))
                }
            }))
            .await;
            stop.cancel();
            let _ = sampler.await;
            let wall = t0.elapsed().as_secs_f64();
            let s = samples.lock().unwrap().clone();
            let kv = s.iter().filter_map(|m| m.kv_usage).fold(0.0f64, f64::max);
            let waiting = s.iter().filter_map(|m| m.waiting).fold(0.0f64, f64::max);
            let p = p95(&lat.iter().map(|x| x.0).collect::<Vec<_>>());
            let tput = lat.iter().map(|x| x.1).sum::<u64>() as f64 / wall;
            if k == 1 {
                single = p;
            }
            if k > 1 && p <= single * 2.2 + 100.0 && waiting == 0.0 && kv < 0.85 {
                recommended = k;
            }
            table.push(json!({"concurrency": k, "p95Ms": p, "throughputTokPerS": (tput * 10.0).round() / 10.0, "peakKv": (kv * 100.0).round() / 100.0, "peakWaiting": waiting}));
        }
        let txt = table.iter().map(|r| format!("k={}: p95 {}ms, {} tok/s, KV {}, waiting {}", r["concurrency"], r["p95Ms"], r["throughputTokPerS"], r["peakKv"], r["peakWaiting"])).collect::<Vec<_>>().join(" | ");
        Ok((Status::Info, format!("recommended FH maxConcurrency (config) = {recommended}. {txt}"), json!({"table": table, "recommended": recommended})))
    })
    .await;

    // 12. cancellation
    probe(&mut results, "cancellation frees the server", async {
        let before = fetch_metrics(cfg, env).await;
        let tok = CancellationToken::new();
        let mut oo = opts(vec![Message::user("Write a very long essay about the history of computing, at least 3000 words.")], Thinking::Off, 4000);
        oo.cancel = Some(tok.clone());
        let l2 = llm.clone();
        let h = tokio::spawn(async move { let _ = l2.chat(oo).await; });
        tokio::time::sleep(Duration::from_millis(1500)).await;
        tok.cancel();
        let _ = h.await;
        let Some(b) = before.and_then(|b| b.running) else { return Ok((Status::Skipped, "no running-requests metric".to_string(), Value::Null)) };
        let mut running = f64::INFINITY;
        for _ in 0..20 {
            if running <= b {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            running = fetch_metrics(cfg, env).await.and_then(|m| m.running).unwrap_or(f64::INFINITY);
        }
        let ok = running <= b;
        Ok((if ok { Status::Pass } else { Status::Fail }, if ok { "aborted request was released within 10s".to_string() } else { format!("still {running} running request(s) 10s after abort") }, json!({"before": b, "after": running})))
    })
    .await;

    // 13. error handling
    probe(&mut results, "error handling (bad model, oversized request)", async {
        let (bs, _) = raw_chat(cfg, env, json!({"model": "no-such-model", "messages": [{"role": "user", "content": "x"}], "max_tokens": 1})).await;
        let big = filler((if max_len > 0 { max_len } else { 32768 } + 20000).max(50000), 0);
        let (hs, hj) = raw_chat(cfg, env, json!({"model": cfg.model, "messages": [{"role": "user", "content": big}], "max_tokens": 1})).await;
        let msg: String = hj["error"]["message"].as_str().unwrap_or("").chars().take(90).collect();
        Ok((if bs >= 400 && hs >= 400 { Status::Pass } else { Status::Warn }, format!("unknown model -> HTTP {bs}; oversized prompt -> HTTP {hs} ({msg})"), json!({"bad": bs, "huge": hs})))
    })
    .await;

    finish(results, cfg, &o)
}

use std::sync::Arc;

fn finish(results: Vec<Probe>, cfg: &Config, o: &ValidateOptions) -> i32 {
    let dir = o.out.join(format!("vllm-{}", chrono_stamp()));
    let _ = std::fs::create_dir_all(&dir);
    let label = |s: Status| match s { Status::Pass => "PASS", Status::Warn => "WARN", Status::Fail => "FAIL", Status::Info => "INFO", Status::Skipped => "SKIP" };
    let mut md = format!("# vLLM validation\n\nModel `{}` at `{}` — {}\n\n| Probe | Result | Summary |\n|---|---|---|\n", cfg.model, cfg.endpoint, chrono_stamp());
    for r in &results {
        md.push_str(&format!("| {} | {} | {} |\n", r.name, label(r.status), r.summary.replace('|', "/")));
    }
    let rep = json!({"model": cfg.model, "endpoint": cfg.endpoint, "at": chrono_stamp(), "results": results.iter().map(|r| json!({"name": r.name, "status": r.status.as_str(), "summary": r.summary, "data": r.data})).collect::<Vec<_>>()});
    let _ = std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&rep).unwrap());
    let _ = std::fs::write(dir.join("report.md"), md);
    let fails = results.iter().filter(|r| r.status == Status::Fail).count();
    let warns = results.iter().filter(|r| r.status == Status::Warn).count();
    println!("\n{fails} failed, {warns} warnings. Report: {}/report.md", dir.display());
    if fails > 0 { 1 } else { 0 }
}
