//! `fh record`: turn a real piece of your own work into an eval task directory.
//! The base revision becomes `repo/`, the prompt and oracle go to `task.json`, and an optional solution
//! revision is stored as `solution.patch` and used to validate the task: the oracle must FAIL on the base
//! and PASS once the solution is applied, otherwise the task cannot tell a good agent from a bad one.
use crate::util::proc::run_simple;
use serde_json::json;
use std::path::{Path, PathBuf};

pub struct RecordOptions {
    pub id: String,
    pub prompt: String,
    pub oracle: String,
    pub out: PathBuf,
    /// git revision of the starting state (default HEAD)
    pub base: Option<String>,
    /// git revision holding the finished change (enables validation and solution.patch)
    pub solution: Option<String>,
    pub setup: Option<String>,
    pub force: bool,
}

#[derive(Debug)]
pub struct RecordReport {
    pub dir: PathBuf,
    pub oracle_fails_on_base: bool,
    pub oracle_passes_with_solution: Option<bool>,
}

async fn export(cwd: &Path, rev: &str, dest: &Path) -> Result<(), String> {
    let _ = std::fs::create_dir_all(dest);
    let r = run_simple(&format!("git archive --format=tar {rev} | tar -x -C {:?}", dest.to_string_lossy()), cwd, 120_000).await;
    if r.code == Some(0) { Ok(()) } else { Err(format!("git archive {rev} failed: {}", r.stderr.trim())) }
}

async fn oracle_passes(dir: &Path, oracle: &str, setup: &Option<String>) -> bool {
    if let Some(s) = setup {
        run_simple(s, dir, 1_800_000).await;
    }
    run_simple(oracle, dir, 600_000).await.code == Some(0)
}

pub async fn record_task(cwd: &Path, o: RecordOptions) -> Result<RecordReport, String> {
    if o.id.is_empty() || o.id.contains('/') || o.id.contains("..") {
        return Err("task id must be a plain name".into());
    }
    if o.prompt.trim().is_empty() || o.oracle.trim().is_empty() {
        return Err("both --prompt and --oracle are required".into());
    }
    let resolve = |rev: String| async move {
        let r = run_simple(&format!("git rev-parse --verify {rev}^{{commit}}"), cwd, 10_000).await;
        if r.code == Some(0) { Ok(r.stdout.trim().to_string()) } else { Err(format!("unknown revision {rev}")) }
    };
    let base = resolve(o.base.clone().unwrap_or_else(|| "HEAD".into())).await?;
    let solution = match &o.solution {
        Some(s) => Some(resolve(s.clone()).await?),
        None => None,
    };
    let dir = o.out.join(&o.id);
    if dir.exists() && !o.force {
        return Err(format!("{} already exists (use --force to replace it)", dir.display()));
    }
    let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
    let before = scratch.path().join("before");
    export(cwd, &base, &before).await?;
    let fails = !oracle_passes(&before, &o.oracle, &o.setup).await;
    let mut passes: Option<bool> = None;
    let mut patch = String::new();
    if let Some(sol) = &solution {
        patch = run_simple(&format!("git diff --binary {base} {sol}"), cwd, 60_000).await.stdout;
        let after = scratch.path().join("after");
        export(cwd, &base, &after).await?;
        let pf = scratch.path().join("solution.patch");
        std::fs::write(&pf, &patch).map_err(|e| e.to_string())?;
        let ap = run_simple(&format!("git init -q && git apply {:?}", pf.to_string_lossy()), &after, 30_000).await;
        passes = Some(ap.code == Some(0) && oracle_passes(&after, &o.oracle, &o.setup).await);
    }
    if !o.force {
        if !fails {
            return Err("the oracle already passes on the base revision: the task would be solved without doing anything".into());
        }
        if passes == Some(false) {
            return Err("the oracle does not pass with the solution applied: the task is not solvable as recorded".into());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    export(cwd, &base, &dir.join("repo")).await?;
    let mut tj = json!({"id": o.id, "prompt": o.prompt, "oracle": o.oracle, "timeoutS": 900, "tags": ["recorded"]});
    if let Some(s) = &o.setup {
        tj["setup"] = json!(s);
    }
    std::fs::write(dir.join("task.json"), serde_json::to_string_pretty(&tj).unwrap_or_default()).map_err(|e| e.to_string())?;
    if !patch.is_empty() {
        std::fs::write(dir.join("solution.patch"), patch).map_err(|e| e.to_string())?;
    }
    Ok(RecordReport { dir, oracle_fails_on_base: fails, oracle_passes_with_solution: passes })
}
