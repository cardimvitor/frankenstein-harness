use anyhow::{anyhow, Result};
use regex::Regex;
use std::path::{Path, PathBuf};

/// Resolve a user/model path inside the workspace; errors when it escapes (including via symlinks).
pub fn in_workspace(root: &Path, p: &str) -> Result<PathBuf> {
    let joined = if Path::new(p).is_absolute() { PathBuf::from(p) } else { root.join(p) };
    let abs = normalize(&joined);
    let root_real = root.canonicalize().map_err(|e| anyhow!("workspace not accessible: {e}"))?;
    let mut probe = abs.clone();
    while !probe.exists() {
        match probe.parent() {
            Some(par) if par != probe => probe = par.to_path_buf(),
            _ => break,
        }
    }
    let real = probe.canonicalize().map_err(|e| anyhow!("cannot resolve {p}: {e}"))?;
    if !real.starts_with(&root_real) {
        return Err(anyhow!("path escapes workspace: {p}"));
    }
    Ok(abs)
}

/// Lexically resolve `.` and `..` (no filesystem access).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Workspace-relative path with forward slashes.
pub fn rel(root: &Path, abs: &Path) -> String {
    let r = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let a = abs.strip_prefix(&r).or_else(|_| abs.strip_prefix(root)).unwrap_or(abs);
    a.to_string_lossy().replace('\\', "/")
}

/// Minimal glob -> Regex (supports **, *, ?).
pub fn glob_to_regex(glob: &str) -> Regex {
    let cs: Vec<char> = glob.chars().collect();
    let mut re = String::from("^");
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '*' => {
                if cs.get(i + 1) == Some(&'*') {
                    re.push_str(".*");
                    i += 1;
                    if cs.get(i + 1) == Some(&'/') {
                        i += 1;
                    }
                } else {
                    re.push_str("[^/]*");
                }
            }
            '?' => re.push_str("[^/]"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push('$');
    Regex::new(&re).unwrap_or_else(|_| Regex::new("^$").unwrap())
}

pub fn matches_any(path: &str, globs: &[String]) -> bool {
    globs.iter().any(|g| {
        let prefix = g.strip_suffix("/**").map(|p| format!("{p}/")).unwrap_or_else(|| g.clone());
        glob_to_regex(g).is_match(path) || path.starts_with(&prefix)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_confinement() {
        let d = tempfile::tempdir().unwrap();
        assert!(in_workspace(d.path(), "a/b.txt").is_ok());
        assert!(in_workspace(d.path(), "../evil").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", d.path().join("link")).unwrap();
            assert!(in_workspace(d.path(), "link/passwd").is_err());
        }
    }

    #[test]
    fn globs() {
        assert!(matches_any("src/a/b.ts", &["src/**".into()]));
        assert!(matches_any("src/a.ts", &["src/*.ts".into()]));
        assert!(!matches_any("srcx/a.ts", &["src/**".into()]));
        assert!(matches_any("test/a.test.js", &["**/*.test.*".into()]));
        assert!(matches_any("lib/a.js", &["lib/a.js".into()]));
    }
}
