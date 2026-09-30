//! Builds SUMMARY.md from the artifacts written by scripts/vps-validate.sh and grades them against the success criteria.
use serde_json::Value;
use std::path::Path;

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn first_dir(dir: &Path, prefix: &str) -> Option<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(prefix) && e.path().is_dir()).map(|e| e.path()).collect();
    v.sort();
    v.pop()
}

fn med(a: &[f64]) -> f64 {
    let mut s = a.to_vec();
    s.sort_by(|x, y| x.partial_cmp(y).unwrap());
    if s.is_empty() { 0.0 } else { s[s.len() / 2] }
}

pub fn summarize_run(dir: &Path, failed_steps: &str) -> String {
    let vllm: Option<Value> = first_dir(dir, "vllm-").and_then(|d| serde_json::from_str(&read(&d.join("report.json"))).ok());
    let rows: Vec<Value> = first_dir(dir, "eval-").map(|d| read(&d.join("results.jsonl")).lines().filter(|l| !l.is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default();
    let unit = read(&dir.join("unit.log"));
    let web = read(&dir.join("web-tests.log"));
    let smoke = read(&dir.join("web-smoke.txt"));
    let probe = |n: &str| vllm.as_ref().and_then(|v| v["results"].as_array()).and_then(|a| a.iter().find(|r| r["name"].as_str().map(|s| s.starts_with(n)).unwrap_or(false))).cloned();

    let mut lines: Vec<String> = Vec::new();
    let mut grade = |name: &str, g: &str, detail: String| lines.push(format!("| {name} | {g} | {} |", detail.replace('|', "/")));

    let tc = probe("tool-call reliability");
    match &tc {
        Some(t) => {
            let mr = t["data"]["malformedRate"].as_f64().unwrap_or(1.0);
            grade("Malformed tool calls < 1%", if mr < 0.01 { "PASS" } else { "FAIL" }, format!("malformed {:.2}%, repaired-or-malformed {:.1}%, {} calls", mr * 100.0, t["data"]["repairedRate"].as_f64().unwrap_or(0.0) * 100.0, t["data"]["total"]));
        }
        None => grade("Malformed tool calls < 1%", "NO DATA", "validate-vllm did not run".into()),
    }
    match probe("thinking must not leak") {
        Some(l) => grade("No thinking leaked into tool args", if l["status"] == "pass" { "PASS" } else { "FAIL" }, l["summary"].as_str().unwrap_or("").into()),
        None => grade("No thinking leaked into tool args", "NO DATA", String::new()),
    }

    let of = |r: &str| -> Vec<&Value> { rows.iter().filter(|x| x["runner"] == r).collect() };
    let (fh, qw) = (of("fh"), of("qwen"));
    let rate = |rs: &[&Value]| if rs.is_empty() { 0.0 } else { rs.iter().filter(|r| r["solved"] == true).count() as f64 / rs.len() as f64 };
    let secs = |rs: &[&Value]| med(&rs.iter().filter_map(|r| r["seconds"].as_f64()).collect::<Vec<_>>());
    if !fh.is_empty() && !qw.is_empty() {
        grade("Pass rate >= plain Qwen Code (same model)", if rate(&fh) >= rate(&qw) { "PASS" } else { "FAIL" }, format!("fh {:.0}% vs qwen {:.0}% over {} runs each (smoke corpus; use SWE-bench/Terminal-Bench subsets for the real margin)", rate(&fh) * 100.0, rate(&qw) * 100.0, fh.len()));
        grade("Median task time no worse", if secs(&fh) <= secs(&qw) { "PASS" } else { "FAIL" }, format!("fh {:.1}s vs qwen {:.1}s (fh includes planning and verification rounds)", secs(&fh), secs(&qw)));
    } else if !fh.is_empty() {
        grade("Pass rate / time vs plain Qwen Code", "NO DATA", format!("fh solved {:.0}% in median {:.1}s; baseline not run", rate(&fh) * 100.0, secs(&fh)));
    } else {
        grade("Pass rate / time vs plain Qwen Code", "NO DATA", "eval did not run".into());
    }
    let single = of("fh-single");
    if !fh.is_empty() && !single.is_empty() {
        // the same rule as `fh eval` prints (docs/ROADMAP.md section 5): pass rate, wall time and uncached tokens per solved task
        let stats = |rs: &[&Value]| crate::eval::runner::RunnerStats {
            n: rs.len(),
            solved: rs.iter().filter(|r| r["solved"] == true).count(),
            median_s: secs(rs),
            cost_per_solved: { let c: f64 = rs.iter().filter_map(|r| r["costTokens"].as_f64()).sum(); let solved = rs.iter().filter(|r| r["solved"] == true).count(); if solved > 0 && rs.iter().any(|r| r["costTokens"].is_number()) { Some(c / solved as f64) } else { None } },
        };
        let rule = crate::eval::runner::fanout_rule(&stats(&fh), &stats(&single));
        let detail = rule.iter().map(|(n, ok, d)| format!("{} {n}: {d}", if *ok { "ok" } else { "FAILED" })).collect::<Vec<_>>().join("; ");
        grade("Orchestration adds value (fh vs fh-single)", if rule.iter().all(|r| r.1) { "PASS" } else { "FAIL" }, format!("{detail}. If FAIL, set maxConcurrency to 1 (docs/ROADMAP.md, gate decision 1)"));
    }
    let gate_ok = unit.contains("test gate_is_deterministic_fast_and_makes_no_llm_call ... ok");
    if !fh.is_empty() {
        let vfn = fh.iter().filter(|r| r["verifierFalseNegative"] == true).count();
        let vfp = fh.iter().filter(|r| r["verdict"] == "fail" && r["solved"] == true).count();
        grade("Verifier false positives < 10%", if (vfp as f64 / fh.len() as f64) < 0.1 { "PASS" } else { "FAIL" }, format!("verdict=fail but oracle passes: {vfp}/{}; verdict=pass but oracle fails: {vfn}/{}", fh.len(), fh.len()));
        let mean_gate = fh.iter().filter_map(|r| r["gateMs"].as_f64()).sum::<f64>() / fh.len() as f64;
        grade("Skill gate adds no LLM call", if gate_ok { "PASS" } else { "NO DATA" }, format!("mean gate {mean_gate:.2} ms (unit test asserts zero LLM calls and p50 < 50 ms)"));
    } else {
        grade("Verifier false positives < 10%", "NO DATA", "eval did not run".into());
        grade("Skill gate adds no LLM call", if gate_ok { "PASS" } else if unit.is_empty() { "NO DATA" } else { "FAIL" }, "unit test".into());
    }
    let web_fail = web.contains("FAILED") || web.contains("test result: FAILED");
    let web_ok = !web.is_empty() && !web_fail && web.contains("test result: ok");
    let smoke_ok = smoke.contains("forged Host -> 403") && smoke.contains("API without cookie -> 401") && smoke.contains("foreign Origin POST -> 403");
    let (web_grade, web_detail) = if smoke.is_empty() && web.is_empty() {
        ("NO DATA", "not run".to_string())
    } else if web.is_empty() {
        (if smoke_ok { "PASS" } else { "FAIL" }, "live HTTP smoke test only (unit tests were skipped)".to_string())
    } else {
        (if web_ok && smoke_ok { "PASS" } else { "FAIL" }, "unit tests + live HTTP smoke test".to_string())
    };
    grade("Web UI security checklist (Host/Origin, cookie, CSP, loopback)", web_grade, web_detail);
    let tui = read(&dir.join("tui-smoke.txt"));
    if !tui.is_empty() {
        let ok = ["HEADER True True", "PLAN True", "VERIFIED True True", "EXIT True", "FILE True"].iter().all(|k| tui.contains(k));
        grade("Terminal UI works in a real terminal", if ok { "PASS" } else { "FAIL" }, "header, plan card, approval, verified result, clean exit (pseudo-terminal, scripted mock model)".into());
    }
    let unit_ok = !unit.is_empty() && !unit.contains("FAILED") && unit.contains("test result: ok");
    let passed: usize = unit.lines().filter_map(|l| l.split("test result: ok. ").nth(1)).filter_map(|r| r.split(" passed").next()?.trim().parse::<usize>().ok()).sum();
    grade("Harness self-tests", if unit.is_empty() { "NO DATA" } else if unit_ok { "PASS" } else { "FAIL" }, format!("{passed} tests passed"));

    let mut out: Vec<String> = vec!["# Frankenstein Harness validation summary".into(), String::new(), format!("Run directory: `{}`", dir.display()), String::new(), "| Criterion | Result | Detail |".into(), "|---|---|---|".into()];
    out.extend(lines);
    out.push(String::new());
    if !failed_steps.trim().is_empty() {
        out.push(format!("Steps that reported problems: {failed_steps}"));
        out.push(String::new());
    }
    if let Some(v) = &vllm {
        out.push("## vLLM measurements".into());
        out.push(String::new());
        out.push("| Probe | Result | Summary |".into());
        out.push("|---|---|---|".into());
        for r in v["results"].as_array().cloned().unwrap_or_default() {
            out.push(format!("| {} | {} | {} |", r["name"].as_str().unwrap_or(""), r["status"].as_str().unwrap_or("").to_uppercase(), r["summary"].as_str().unwrap_or("").replace('|', "/")));
        }
        out.push(String::new());
        if let Some(c) = probe("concurrency").and_then(|c| c["data"]["recommended"].as_u64()) {
            out.push(format!("Suggested `maxConcurrency` for `.fh/config.json`: **{c}**"));
            out.push(String::new());
        }
    }
    if let Some(d) = first_dir(dir, "eval-") {
        let s = read(&d.join("summary.md"));
        if !s.is_empty() {
            out.push("## Eval".into());
            out.push(String::new());
            out.push(s.lines().skip(1).collect::<Vec<_>>().join("\n"));
            out.push(String::new());
        }
    }
    if let Some(v) = &vllm {
        out.push("## Suggested vLLM and harness changes".into());
        out.push(String::new());
        for r in crate::validate::recommend::recommend(v) {
            out.push(format!("- {r}"));
        }
        out.push(String::new());
    }
    out.push("## Not covered by this run".into());
    out.push(String::new());
    out.push("- SWE-bench Verified / Terminal-Bench subsets and your recorded .NET/React/Angular tasks (load with `--tasks <dir>`, see docs/EVAL.md): the built-in corpus is a smoke test.".into());
    out.push("- Windows and macOS execution (run the same script on those hosts; Windows sandboxing is not implemented).".into());
    out.push(String::new());
    out.join("\n")
}
