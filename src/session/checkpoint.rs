use crate::util::proc::{run, scrubbed_env, RunOpts};
use std::path::{Path, PathBuf};

/// Snapshots and diffs ignore the harness's own state and generated artifacts (test runs create these; they are not task output).
const ADD_ALL: &str = "git add -A -- . ':(exclude).fh' ':(exclude,glob)**/__pycache__/**' ':(exclude,glob)**/*.pyc' ':(exclude,glob)**/.pytest_cache/**' ':(exclude,glob)**/.mypy_cache/**' ':(exclude,glob)**/node_modules/**'";

#[derive(Default, Debug, Clone)]
pub struct Changes {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

impl Changes {
    pub fn all(&self) -> Vec<String> {
        self.added.iter().chain(self.modified.iter()).chain(self.deleted.iter()).cloned().collect()
    }
}

/// Git-backed per-turn snapshots that do not touch the user's index, branch or stash.
#[derive(Clone)]
pub struct Checkpoints {
    cwd: PathBuf,
}

struct TempIndex(PathBuf);
impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Checkpoints {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Checkpoints { cwd: cwd.into() }
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub async fn is_repo(&self) -> bool {
        run("git rev-parse --is-inside-work-tree", &self.cwd, RunOpts::default()).await.code == Some(0)
    }

    fn temp_env(&self) -> (TempIndex, Vec<(String, String)>) {
        let dir = std::env::temp_dir().join(format!("fh-idx-{}-{}", std::process::id(), rand::random::<u64>()));
        let _ = std::fs::create_dir_all(&dir);
        let mut env = scrubbed_env();
        env.push(("GIT_INDEX_FILE".into(), dir.join("index").to_string_lossy().into()));
        for (k, v) in [("GIT_AUTHOR_NAME", "fh"), ("GIT_AUTHOR_EMAIL", "fh@local"), ("GIT_COMMITTER_NAME", "fh"), ("GIT_COMMITTER_EMAIL", "fh@local")] {
            env.push((k.into(), v.into()));
        }
        (TempIndex(dir), env)
    }

    async fn git(&self, cmd: &str, env: &[(String, String)]) -> crate::util::proc::RunResult {
        run(cmd, &self.cwd, RunOpts { env: Some(env.to_vec()), ..Default::default() }).await
    }

    /// Snapshot the working tree (untracked included, .gitignore respected). Returns the commit id.
    pub async fn create(&self, label: &str) -> Option<String> {
        if !self.is_repo().await {
            return None;
        }
        let (_g, env) = self.temp_env();
        if self.git(ADD_ALL, &env).await.code != Some(0) {
            return None;
        }
        let tree = self.git("git write-tree", &env).await.stdout.trim().to_string();
        if tree.is_empty() {
            return None;
        }
        let head = self.git("git rev-parse -q --verify HEAD", &env).await;
        let parent = if head.code == Some(0) { format!("-p {}", head.stdout.trim()) } else { String::new() };
        let c = self.git(&format!("git commit-tree {tree} {parent} -m {:?}", format!("fh checkpoint: {label}")), &env).await;
        let id = c.stdout.trim().to_string();
        if id.is_empty() {
            return None;
        }
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        self.git(&format!("git update-ref refs/fh/checkpoints/{ts} {id}"), &env).await;
        Some(id)
    }

    pub async fn changed_since(&self, id: &str) -> Changes {
        let (_g, env) = self.temp_env();
        self.git(ADD_ALL, &env).await;
        let r = self.git(&format!("git diff --cached --name-status {id} --"), &env).await;
        let mut ch = Changes::default();
        for l in r.stdout.lines().filter(|l| !l.is_empty()) {
            let mut parts = l.split('\t');
            let s = parts.next().unwrap_or("");
            let Some(f) = parts.last() else { continue };
            match s.chars().next() {
                Some('A') => ch.added.push(f.to_string()),
                Some('D') => ch.deleted.push(f.to_string()),
                _ => ch.modified.push(f.to_string()),
            }
        }
        ch
    }

    /// Unified diff of the working tree vs the checkpoint (new files included).
    pub async fn diff_since(&self, id: &str) -> String {
        let (_g, env) = self.temp_env();
        self.git(ADD_ALL, &env).await;
        self.git(&format!("git diff --cached --no-color -U3 {id} --"), &env).await.stdout
    }

    /// Binary-safe patch of the working tree vs the checkpoint (for applicability checks).
    pub async fn patch_since(&self, id: &str) -> String {
        let (_g, env) = self.temp_env();
        self.git(ADD_ALL, &env).await;
        self.git(&format!("git diff --cached --binary --no-color {id} --"), &env).await.stdout
    }

    /// Does the checkpoint commit still exist?
    pub async fn exists(&self, id: &str) -> bool {
        run(&format!("git cat-file -e {id}^{{commit}}"), &self.cwd, RunOpts::default()).await.code == Some(0)
    }

    /// Content of `path` at the checkpoint (None when it did not exist there).
    pub async fn file_at(&self, id: &str, path: &str) -> Option<String> {
        let r = run(&format!("git show {id}:{path:?}"), &self.cwd, RunOpts::default()).await;
        if r.code == Some(0) { Some(r.stdout) } else { None }
    }

    /// Does `patch` apply cleanly to the checkpoint's tree? Checked against a temporary index; the worktree is untouched.
    pub async fn patch_applies(&self, base: &str, patch: &str) -> Result<(), String> {
        if patch.trim().is_empty() {
            return Ok(());
        }
        let (g, env) = self.temp_env();
        let r = self.git(&format!("git read-tree {base}"), &env).await;
        if r.code != Some(0) {
            return Err(format!("cannot read base tree: {}", r.stderr.trim()));
        }
        let file = g.0.join("task.patch");
        std::fs::write(&file, patch).map_err(|e| e.to_string())?;
        let r = self.git(&format!("git apply --check --cached {:?}", file.to_string_lossy()), &env).await;
        if r.code == Some(0) { Ok(()) } else { Err(r.stderr.trim().to_string()) }
    }

    /// Restore the working tree to the checkpoint: revert modifications, restore deletions, remove additions.
    pub async fn restore(&self, id: &str) -> Vec<String> {
        let ch = self.changed_since(id).await;
        for f in ch.modified.iter().chain(ch.deleted.iter()) {
            run(&format!("git checkout {id} -- {f:?}"), &self.cwd, RunOpts::default()).await;
        }
        for f in &ch.added {
            let _ = std::fs::remove_file(self.cwd.join(f));
        }
        ch.all()
    }
}
