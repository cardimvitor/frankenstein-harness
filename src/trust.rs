//! Per-repository trust: project hooks and project MCP servers run only in a workspace the user
//! explicitly trusted (`fh trust`). Stored in the user data dir, keyed by canonical path.
use crate::config::{data_dir, Env};
use std::path::{Path, PathBuf};

fn file(env: &Env) -> PathBuf {
    data_dir(env).join("trust.json")
}

fn key(cwd: &Path) -> String {
    cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()).to_string_lossy().to_string()
}

fn load(env: &Env) -> Vec<String> {
    std::fs::read_to_string(file(env)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(env: &Env, v: &[String]) -> std::io::Result<()> {
    let f = file(env);
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(f, serde_json::to_string_pretty(v).unwrap_or_default())
}

pub fn is_trusted(env: &Env, cwd: &Path) -> bool {
    load(env).contains(&key(cwd))
}

pub fn trust(env: &Env, cwd: &Path) -> std::io::Result<()> {
    let mut v = load(env);
    let k = key(cwd);
    if !v.contains(&k) {
        v.push(k);
    }
    save(env, &v)
}

pub fn untrust(env: &Env, cwd: &Path) -> std::io::Result<()> {
    let k = key(cwd);
    let v: Vec<String> = load(env).into_iter().filter(|x| *x != k).collect();
    save(env, &v)
}

pub fn list(env: &Env) -> Vec<String> {
    load(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trust_roundtrip() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let mut env = Env::new();
        env.insert("FH_HOME".into(), home.path().to_string_lossy().to_string());
        assert!(!is_trusted(&env, ws.path()));
        trust(&env, ws.path()).unwrap();
        assert!(is_trusted(&env, ws.path()));
        untrust(&env, ws.path()).unwrap();
        assert!(!is_trusted(&env, ws.path()));
    }
}
