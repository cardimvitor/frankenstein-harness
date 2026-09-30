use fh::config::{load_config, Config, Env};
use fh::testkit::{self, Scripted};
use fh::ui::cli::{main as cli_main, parse_args};
use fh::validate::vllm::{filler, validate_vllm, ValidateOptions};
use serde_json::{json, Value};
use std::path::Path;

fn cfg(url: &str, base: &str) -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.endpoint = url.into();
    c.metrics_url = format!("{base}/metrics");
    c.model = "mock-qwen".into();
    c.retries = 0;
    c
}

#[test]
fn filler_is_deterministic_and_about_the_requested_size() {
    assert_eq!(filler(1000, 3), filler(1000, 3));
    let n = filler(1000, 3).len();
    assert!(n > 3000 && n < 4200, "{n}");
}

#[tokio::test]
async fn validate_vllm_runs_every_probe_against_a_scripted_server_and_writes_reports() {
    let m = testkit::start(0, None).await;
    testkit::install_smart(&m);
    let out = tempfile::tempdir().unwrap();
    let code = validate_vllm(&cfg(&m.url, &m.base), &Env::new(), ValidateOptions { out: out.path().to_path_buf(), quick: true, trials: Some(10), max_context: Some(16000), concurrency: Some(2) }).await;
    let dir = std::fs::read_dir(out.path()).unwrap().next().unwrap().unwrap().path();
    let rep: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    let get = |prefix: &str| rep["results"].as_array().unwrap().iter().find(|r| r["name"].as_str().unwrap().starts_with(prefix)).unwrap_or_else(|| panic!("no probe {prefix}")).clone();
    let fails: Vec<&Value> = rep["results"].as_array().unwrap().iter().filter(|r| r["status"] == "fail").collect();
    assert_eq!(code, 0, "{fails:?}");
    assert_eq!(get("connectivity and auth")["status"], "warn"); // the mock has no auth: reported as unprotected
    assert!(get("connectivity and auth")["summary"].as_str().unwrap().contains("NOT protected"));
    assert_eq!(get("metrics endpoint")["status"], "pass");
    assert!(get("tool-call reliability")["summary"].as_str().unwrap().contains("malformed 0.0%"));
    assert_eq!(get("tool-call reliability")["data"]["right"], get("tool-call reliability")["data"]["total"]);
    assert_eq!(get("thinking must not leak")["status"], "pass");
    assert_eq!(get("structured JSON output with thinking on")["status"], "pass");
    assert_eq!(get("streamed vs non-streamed")["status"], "pass");
    assert!(get("MTP acceptance")["summary"].as_str().unwrap().contains("66.7%")); // 20/30 accepted
    assert_eq!(get("prefix cache")["status"], "pass");
    assert!(get("long-context")["summary"].as_str().unwrap().contains("ok") && !get("long-context")["summary"].as_str().unwrap().contains("MISS"));
    assert!(get("concurrency and KV")["data"]["recommended"].as_u64().unwrap() >= 1);
    assert!(std::fs::read_to_string(dir.join("report.md")).unwrap().contains("| tool-call reliability"));
}

#[tokio::test]
async fn validate_fails_fast_when_the_model_is_not_served() {
    let m = testkit::start(0, None).await;
    let mut c = cfg(&m.url, &m.base);
    c.model = "wrong-model".into();
    let out = tempfile::tempdir().unwrap();
    assert_eq!(validate_vllm(&c, &Env::new(), ValidateOptions { out: out.path().to_path_buf(), quick: true, ..Default::default() }).await, 1);
    let dir = std::fs::read_dir(out.path()).unwrap().next().unwrap().unwrap().path();
    let rep: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert!(rep["results"][0]["summary"].as_str().unwrap().contains("not served; available: mock-qwen"));
    let _ = (json!(1), Scripted::text(""));
}

#[test]
fn cli_argument_parsing() {
    let a = parse_args(&["run", "fix", "the", "bug", "--auto", "--mode", "yolo", "--cwd=/tmp/x", "--json"].iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(a.cmd, "run");
    assert_eq!(a.positional, vec!["fix", "the", "bug"]);
    assert!(a.has("auto") && a.has("json"));
    assert_eq!(a.get("mode"), Some("yolo"));
    assert_eq!(a.get("cwd"), Some("/tmp/x"));
}

#[tokio::test]
async fn cli_help_and_bad_usage_return_codes() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(cli_main(s(&["--help"])).await, 0);
    assert_eq!(cli_main(s(&["run"])).await, 2);
    assert_eq!(cli_main(s(&["run", "x", "--mode", "nope"])).await, 2);
    assert_eq!(cli_main(s(&["bogus"])).await, 2);
}
