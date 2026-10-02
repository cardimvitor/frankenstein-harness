use crate::config::{auth_headers, Config, Env};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

#[derive(Clone, Debug, Default)]
pub struct VllmMetrics {
    pub kv_usage: Option<f64>,
    pub running: Option<f64>,
    pub waiting: Option<f64>,
    pub prefix_hits: Option<f64>,
    pub prefix_queries: Option<f64>,
    pub spec_drafts: Option<f64>,
    pub spec_accepted: Option<f64>,
    pub spec_draft_tokens: Option<f64>,
    /// accepted tokens per draft position (index 0 = first draft token)
    pub spec_accepted_per_pos: Option<Vec<f64>>,
    pub raw: HashMap<String, f64>,
}

static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([a-zA-Z_:][\w:]*)(\{[^}]*\})?\s+([-+eE.\dNaInf]+)").unwrap());
static POS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"position="(\d+)""#).unwrap());
static PER_POS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"spec_decode_num_accepted_tokens_per_pos(?:_total)?#pos(\d+)$").unwrap());

/// Parse Prometheus text, summing series that share a metric name (per-position series keep their position).
pub fn parse_prometheus(text: &str) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(m) = LINE.captures(line) else { continue };
        let Ok(v) = m[3].parse::<f64>() else { continue };
        if !v.is_finite() {
            continue;
        }
        let key = match m.get(2).and_then(|l| POS.captures(l.as_str())) {
            Some(p) => format!("{}#pos{}", &m[1], &p[1]),
            None => m[1].to_string(),
        };
        *out.entry(key).or_insert(0.0) += v;
    }
    out
}

fn pick(raw: &HashMap<String, f64>, names: &[&str]) -> Option<f64> {
    for n in names {
        for k in [n.to_string(), n.trim_end_matches("_total").to_string()] {
            if let Some(v) = raw.get(&k) {
                return Some(*v);
            }
        }
    }
    None
}

pub fn summarize(raw: HashMap<String, f64>) -> VllmMetrics {
    let mut per: Vec<(usize, f64)> = Vec::new();
    for (k, v) in &raw {
        if let Some(m) = PER_POS.captures(k) {
            // draft positions are single digits in practice; a bogus label must not size a huge vector
            per.push((m[1].parse::<usize>().unwrap_or(0).min(63), *v));
        }
    }
    let per_pos = if per.is_empty() {
        None
    } else {
        let n = per.iter().map(|(i, _)| *i).max().unwrap() + 1;
        let mut v = vec![0.0; n];
        for (i, x) in per {
            v[i] = x;
        }
        Some(v)
    };
    VllmMetrics {
        kv_usage: pick(&raw, &["vllm:kv_cache_usage_perc", "vllm:gpu_cache_usage_perc"]),
        running: pick(&raw, &["vllm:num_requests_running"]),
        waiting: pick(&raw, &["vllm:num_requests_waiting"]),
        prefix_hits: pick(&raw, &["vllm:prefix_cache_hits_total", "vllm:gpu_prefix_cache_hits_total"]),
        prefix_queries: pick(&raw, &["vllm:prefix_cache_queries_total", "vllm:gpu_prefix_cache_queries_total"]),
        spec_drafts: pick(&raw, &["vllm:spec_decode_num_drafts_total"]),
        spec_accepted: pick(&raw, &["vllm:spec_decode_num_accepted_tokens_total"]),
        spec_draft_tokens: pick(&raw, &["vllm:spec_decode_num_draft_tokens_total"]),
        spec_accepted_per_pos: per_pos,
        raw,
    }
}

pub async fn fetch_metrics(cfg: &Config, env: &Env) -> Option<VllmMetrics> {
    let client = crate::http::client_builder(cfg).ok()?.timeout(Duration::from_secs(3)).build().ok()?;
    let mut req = client.get(&cfg.metrics_url);
    for (k, v) in auth_headers(cfg, env) {
        req = req.header(k, v);
    }
    let res = req.send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    Some(summarize(parse_prometheus(&res.text().await.ok()?)))
}

#[derive(Clone, Debug, Default)]
pub struct MetricsDelta {
    pub prefix_hit_rate: Option<f64>,
    pub acceptance_rate: Option<f64>,
    pub mean_accepted_per_draft: Option<f64>,
    pub per_position_acceptance: Option<Vec<f64>>,
}

/// Deltas between two snapshots (counters are cumulative).
pub fn delta(a: &Option<VllmMetrics>, b: &Option<VllmMetrics>) -> MetricsDelta {
    let (Some(a), Some(b)) = (a, b) else { return MetricsDelta::default() };
    let mut d = MetricsDelta::default();
    if let (Some(aq), Some(bq), Some(ah), Some(bh)) = (a.prefix_queries, b.prefix_queries, a.prefix_hits, b.prefix_hits) {
        if bq - aq > 0.0 {
            d.prefix_hit_rate = Some((bh - ah) / (bq - aq));
        }
    }
    if let (Some(adt), Some(bdt), Some(aa), Some(ba)) = (a.spec_draft_tokens, b.spec_draft_tokens, a.spec_accepted, b.spec_accepted) {
        if bdt - adt > 0.0 {
            d.acceptance_rate = Some((ba - aa) / (bdt - adt));
        }
    }
    if let (Some(ad), Some(bd), Some(aa), Some(ba)) = (a.spec_drafts, b.spec_drafts, a.spec_accepted, b.spec_accepted) {
        let dd = bd - ad;
        if dd > 0.0 {
            d.mean_accepted_per_draft = Some((ba - aa) / dd);
            if let Some(bp) = &b.spec_accepted_per_pos {
                d.per_position_acceptance = Some(bp.iter().enumerate().map(|(i, v)| (v - a.spec_accepted_per_pos.as_ref().and_then(|x| x.get(i)).copied().unwrap_or(0.0)) / dd).collect());
            }
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_delta() {
        let t1 = "vllm:prefix_cache_queries_total{model=\"m\"} 100\nvllm:prefix_cache_hits_total{model=\"m\"} 10\nvllm:spec_decode_num_drafts_total 10\nvllm:spec_decode_num_draft_tokens_total 30\nvllm:spec_decode_num_accepted_tokens_total 12\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position=\"0\"} 8\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position=\"1\"} 3\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position=\"2\"} 1\nvllm:kv_cache_usage_perc 0.4\n";
        let t2 = "vllm:prefix_cache_queries_total{model=\"m\"} 200\nvllm:prefix_cache_hits_total{model=\"m\"} 60\nvllm:spec_decode_num_drafts_total 20\nvllm:spec_decode_num_draft_tokens_total 60\nvllm:spec_decode_num_accepted_tokens_total 30\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position=\"0\"} 17\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position=\"1\"} 3\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position=\"2\"} 1\nvllm:kv_cache_usage_perc 0.4\n";
        let a = summarize(parse_prometheus(t1));
        let b = summarize(parse_prometheus(t2));
        assert_eq!(a.kv_usage, Some(0.4));
        assert_eq!(a.spec_accepted_per_pos, Some(vec![8.0, 3.0, 1.0]));
        let d = delta(&Some(a), &Some(b));
        assert_eq!(d.acceptance_rate, Some(18.0 / 30.0));
        assert_eq!(d.prefix_hit_rate, Some(0.5));
        assert_eq!(d.mean_accepted_per_draft, Some(1.8));
        assert_eq!(d.per_position_acceptance.unwrap()[0], 0.9);
    }
}
