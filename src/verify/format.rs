//! Formatter arbiter: autodetected formatters run in check mode on the changed files only.
//! A file that was already unformatted at the base checkpoint is not blamed on the task, so a
//! messy repository never fails every task. Only configured, installed formatters run.
use super::checks::{CheckKind, CheckResult, Status};
use crate::session::checkpoint::Checkpoints;
use crate::util::proc::{clip, run, RunOpts};
use std::path::Path;
use std::time::Duration;

struct Fmt {
    name: &'static str,
    exts: &'static [&'static str],
    /// command prefix; files are appended, shell-quoted
    cmd: &'static str,
    /// gofmt lists unformatted files on stdout instead of failing
    stdout_means_fail: bool,
    applicable: fn(&Path) -> bool,
}

fn exists_any(cwd: &Path, names: &[&str]) -> bool {
    names.iter().any(|n| cwd.join(n).exists())
}

fn pkg_has(cwd: &Path, key: &str) -> bool {
    std::fs::read_to_string(cwd.join("package.json")).map(|s| s.contains(&format!("\"{key}\""))).unwrap_or(false)
}

fn toml_has(cwd: &Path, file: &str, needle: &str) -> bool {
    std::fs::read_to_string(cwd.join(file)).map(|s| s.contains(needle)).unwrap_or(false)
}

fn on_path(bin: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    std::process::Command::new(finder).arg(bin).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn formatters() -> Vec<Fmt> {
    vec![
        Fmt { name: "rustfmt", exts: &["rs"], cmd: "rustfmt --check --edition 2021", stdout_means_fail: false, applicable: |d| d.join("Cargo.toml").exists() && on_path("rustfmt") },
        Fmt { name: "ruff format", exts: &["py"], cmd: "ruff format --check", stdout_means_fail: false, applicable: |d| (exists_any(d, &["ruff.toml", ".ruff.toml"]) || toml_has(d, "pyproject.toml", "[tool.ruff")) && on_path("ruff") },
        Fmt { name: "black", exts: &["py"], cmd: "black --check -q", stdout_means_fail: false, applicable: |d| toml_has(d, "pyproject.toml", "[tool.black]") && on_path("black") },
        Fmt {
            name: "prettier",
            exts: &["js", "jsx", "ts", "tsx", "css", "scss", "json", "html", "md", "vue"],
            cmd: "node_modules/.bin/prettier --check --log-level warn",
            stdout_means_fail: false,
            applicable: |d| d.join("node_modules/.bin/prettier").exists() && (exists_any(d, &[".prettierrc", ".prettierrc.json", ".prettierrc.yaml", ".prettierrc.yml", ".prettierrc.js", "prettier.config.js", "prettier.config.mjs"]) || pkg_has(d, "prettier")),
        },
        Fmt { name: "gofmt", exts: &["go"], cmd: "gofmt -l", stdout_means_fail: true, applicable: |d| d.join("go.mod").exists() && on_path("gofmt") },
        Fmt { name: "dotnet format", exts: &["cs"], cmd: "dotnet format --verify-no-changes --no-restore --include", stdout_means_fail: false, applicable: |d| d.join(".editorconfig").exists() && on_path("dotnet") },
    ]
}

fn quote(f: &str) -> String {
    format!("'{}'", f.replace('\'', "'\\''"))
}

async fn bad_files(cwd: &Path, f: &Fmt, files: &[String]) -> Vec<String> {
    if files.is_empty() {
        return vec![];
    }
    let args: Vec<String> = files.iter().map(|x| quote(x)).collect();
    let r = run(&format!("{} {}", f.cmd, args.join(" ")), cwd, RunOpts { timeout: Some(Duration::from_secs(120)), ..Default::default() }).await;
    let failed = if f.stdout_means_fail { !r.stdout.trim().is_empty() } else { r.code != Some(0) };
    if !failed {
        return vec![];
    }
    // Per-file attribution: re-run individually so one bad file does not hide which one it is.
    if files.len() == 1 {
        return files.to_vec();
    }
    let mut out = Vec::new();
    for x in files {
        let r = run(&format!("{} {}", f.cmd, quote(x)), cwd, RunOpts { timeout: Some(Duration::from_secs(60)), ..Default::default() }).await;
        let failed = if f.stdout_means_fail { !r.stdout.trim().is_empty() } else { r.code != Some(0) };
        if failed {
            out.push(x.clone());
        }
    }
    out
}

/// Runs every applicable formatter over the changed (added or modified) files. None when nothing applies.
pub async fn format_check(cwd: &Path, cp: &Checkpoints, base: &str, changed: &[String]) -> Option<CheckResult> {
    let t0 = std::time::Instant::now();
    let mut problems: Vec<String> = Vec::new();
    let mut ran: Vec<&str> = Vec::new();
    for f in formatters() {
        if !(f.applicable)(cwd) {
            continue;
        }
        let files: Vec<String> = changed.iter().filter(|p| cwd.join(p).is_file() && Path::new(p).extension().and_then(|e| e.to_str()).map(|e| f.exts.contains(&e)).unwrap_or(false)).cloned().collect();
        if files.is_empty() {
            continue;
        }
        ran.push(f.name);
        for bad in bad_files(cwd, &f, &files).await {
            // blame the task only if the file was clean at the base (or did not exist there)
            let was_clean = match cp.file_at(base, &bad).await {
                None => true,
                Some(content) => {
                    let mirror = cwd.join(".fh").join("basefmt").join(&bad);
                    if let Some(d) = mirror.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::write(&mirror, content);
                    let rel = format!(".fh/basefmt/{bad}");
                    let clean = bad_files(cwd, &f, &[rel]).await.is_empty();
                    let _ = std::fs::remove_file(&mirror);
                    clean
                }
            };
            if was_clean {
                problems.push(format!("{bad}: not formatted according to {} (run the formatter on this file)", f.name));
            }
        }
    }
    let _ = std::fs::remove_dir_all(cwd.join(".fh").join("basefmt"));
    if ran.is_empty() {
        return None;
    }
    Some(CheckResult {
        name: "format".into(),
        kind: CheckKind::Lint,
        status: if problems.is_empty() { Status::Pass } else { Status::Fail },
        detail: if problems.is_empty() { format!("formatted ({})", ran.join(", ")) } else { clip(&problems.join("\n"), 3000) },
        ms: t0.elapsed().as_millis() as u64,
    })
}
