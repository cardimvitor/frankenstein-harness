//! Turns a `fh validate-vllm` report into concrete vLLM/harness changes. Rule-based on purpose: every suggestion
//! cites the measurement that triggered it, and none of them is applied automatically.
use serde_json::Value;

fn probe<'a>(rep: &'a Value, prefix: &str) -> Option<&'a Value> {
    rep["results"].as_array()?.iter().find(|r| r["name"].as_str().map(|n| n.starts_with(prefix)).unwrap_or(false))
}

pub fn recommend(rep: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: String| out.push(s);

    if let Some(c) = probe(rep, "connectivity") {
        if c["data"]["unauthenticatedStatus"] == 200 {
            add("The endpoint answers without credentials: start vLLM with an API key (`VLLM_API_KEY` env var) and keep it bound to 127.0.0.1 or behind a firewall.".into());
        }
        if c["data"]["vllmVersion"].is_null() {
            add("`/version` was not readable, so the vLLM version is unknown; note it in your validation record (Blackwell support and MTP behaviour differ across releases).".into());
        }
    }
    if let Some(s) = probe(rep, "streaming speed") {
        if s["data"]["off"]["reasoningField"] == true {
            add("Thinking-off requests still returned reasoning: the chat template may ignore `enable_thinking`. Check the model's chat template and the `--reasoning-parser`.".into());
        }
        if s["data"]["high"]["reasoningField"] == false {
            add("Thinking-on returned no `reasoning_content`: add `--reasoning-parser qwen3` (or the parser that matches this model generation).".into());
        }
    }
    if let Some(t) = probe(rep, "tool-call reliability") {
        let (mal, rep_rate, leak) = (t["data"]["malformedRate"].as_f64().unwrap_or(0.0), t["data"]["repairedRate"].as_f64().unwrap_or(0.0), t["data"]["leak"].as_u64().unwrap_or(0));
        if mal >= 0.01 || rep_rate > 0.05 {
            add(format!("Tool calls needed repair {:.1}% of the time ({:.1}% unusable): confirm `--tool-call-parser` matches the model (qwen3_coder for Qwen3-Coder style XML calls), keep tool-turn temperature at or below 0.6, and re-measure. If it persists, MTP may be perturbing long JSON: compare with `num_speculative_tokens` 2.", rep_rate * 100.0, mal * 100.0));
        }
        if leak > 0 {
            add("Reasoning text leaked into tool arguments: the reasoning parser is not stripping the think block before the tool parser runs; upgrade vLLM or switch parser combination.".into());
        }
    }
    if let Some(s) = probe(rep, "structured JSON") {
        if s["data"]["viaFallback"].as_u64().unwrap_or(0) > 0 {
            add("Structured output with thinking only works through the harness's fallback: this vLLM applies guided decoding before the reasoning block ends. Planner and reviewer lose thinking on those calls; a newer vLLM with reasoning-aware structured outputs fixes it.".into());
        }
    }
    if let Some(s) = probe(rep, "streamed vs non-streamed") {
        if s["status"] != "pass" {
            add("Streamed and non-streamed tool calls differ at temperature 0: speculative decoding should be output-neutral, so treat this as a bug signal. Retest with speculative decoding off to isolate it, and pin the vLLM version until fixed.".into());
        }
    }
    if let Some(m) = probe(rep, "MTP acceptance") {
        let per = &m["data"]["code"]["perPosition"];
        if let Some(p) = per.as_array().filter(|a| a.len() >= 3) {
            let last = p[p.len() - 1].as_f64().unwrap_or(1.0);
            let first = p[0].as_f64().unwrap_or(0.0);
            if last < 0.30 {
                add(format!("Draft position {} is accepted only {:.0}% of the time (position 1: {:.0}%): the extra draft token costs more than it saves. Try `num_speculative_tokens` {}.", p.len(), last * 100.0, first * 100.0, p.len() - 1));
            }
        }
        let (prose, code, json) = (m["data"]["prose"]["acceptanceRate"].as_f64(), m["data"]["code"]["acceptanceRate"].as_f64(), m["data"]["json"]["acceptanceRate"].as_f64());
        if let (Some(p), Some(c)) = (prose, code) {
            if c < p {
                add("Acceptance on code is lower than on prose, the opposite of what the harness's structured-output design assumes; check sampling (lower temperature raises acceptance) before changing the harness.".into());
            }
        }
        if json.map(|j| j < 0.35).unwrap_or(false) {
            add("Acceptance on JSON output is below 35%: MTP is not paying off for tool calls on this setup; compare end-to-end task time with speculative decoding off.".into());
        }
    }
    if let Some(p) = probe(rep, "prefix cache") {
        if p["status"] == "warn" {
            add("Prefix caching is not visibly effective: make sure `--enable-prefix-caching` is set (it is the default in recent vLLM V1, but check), and that the metrics endpoint is the same server that serves requests.".into());
        }
    }
    if let Some(l) = probe(rep, "long-context") {
        if let Some(rows) = l["data"]["rows"].as_array() {
            if let Some(bad) = rows.iter().find(|r| r["correct"] != true) {
                let good = rows.iter().take_while(|r| r["correct"] == true).last().and_then(|r| r["promptTokens"].as_u64()).unwrap_or(0);
                add(format!("Retrieval failed at ~{} tokens (last good size: {} tokens). Set `contextWindow` in the harness config to about {} and consider an unquantized KV cache (`--kv-cache-dtype auto`) if fp8 KV is enabled.", bad["promptTokens"], good, good));
            }
        }
    }
    if let Some(c) = probe(rep, "concurrency") {
        if let Some(rec) = c["data"]["recommended"].as_u64() {
            add(format!("Set `maxConcurrency` to {rec} in `.fh/config.json` (highest parallel load that kept p95 latency near the single-request baseline with no queueing)."));
        }
        if let Some(rows) = c["data"]["table"].as_array() {
            if rows.iter().any(|r| r["peakKv"].as_f64().unwrap_or(0.0) > 0.9) {
                add("KV cache exceeded 90% under parallel load: raise `--gpu-memory-utilization` (if there is headroom), use `--kv-cache-dtype fp8`, or lower `--max-num-seqs` / the harness's `maxConcurrency`.".into());
            }
        }
    }
    if let Some(c) = probe(rep, "cancellation") {
        if c["status"] == "fail" {
            add("Aborted requests were not released by vLLM within 10 seconds: upgrade vLLM (client-disconnect handling), otherwise cancelled tasks will keep occupying the GPU.".into());
        }
    }
    if out.is_empty() {
        out.push("No vLLM changes suggested by this run.".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rules_fire_on_bad_measurements_and_stay_quiet_on_good_ones() {
        let bad = json!({"results": [
            {"name": "connectivity and auth", "data": {"unauthenticatedStatus": 200, "vllmVersion": null}},
            {"name": "streaming speed (thinking off/on)", "data": {"off": {"reasoningField": false}, "high": {"reasoningField": false}}},
            {"name": "tool-call reliability (10 calls)", "data": {"malformedRate": 0.03, "repairedRate": 0.2, "leak": 2}},
            {"name": "MTP acceptance (structured vs prose)", "data": {"code": {"acceptanceRate": 0.5, "perPosition": [0.8, 0.5, 0.1]}, "prose": {"acceptanceRate": 0.6}, "json": {"acceptanceRate": 0.3}}},
            {"name": "prefix cache (x)", "status": "warn"},
            {"name": "long-context retrieval", "data": {"rows": [{"promptTokens": 4000, "correct": true}, {"promptTokens": 32000, "correct": false}]}},
            {"name": "concurrency and KV pressure", "data": {"recommended": 3, "table": [{"peakKv": 0.95}]}},
            {"name": "cancellation frees the server", "status": "fail"}
        ]});
        let r = recommend(&bad).join("\n");
        for want in ["API key", "--reasoning-parser", "--tool-call-parser", "Draft position 3", "num_speculative_tokens", "prefix", "contextWindow", "maxConcurrency", "gpu-memory-utilization", "upgrade vLLM"] {
            assert!(r.contains(want), "missing {want:?} in:\n{r}");
        }
        let good = json!({"results": [{"name": "tool-call reliability", "data": {"malformedRate": 0.0, "repairedRate": 0.0, "leak": 0}}]});
        assert_eq!(recommend(&good), vec!["No vLLM changes suggested by this run.".to_string()]);
    }
}
