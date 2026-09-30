use crate::funnel::intake::Subtask;
use std::collections::HashSet;

fn prefix(g: &str) -> String {
    let cut = g.find('*').map(|i| &g[..i]).unwrap_or(g);
    let mut p = cut.trim_end_matches('/').to_string();
    p.push('/');
    p.trim_start_matches("./").to_string()
}

fn is_file(g: &str) -> bool {
    !g.contains('*') && g.rsplit('/').next().map(|f| f.contains('.') && !f.ends_with('.')).unwrap_or(false)
}

/// Two ownership globs overlap when one directory prefix contains the other or the paths are identical.
pub fn globs_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if is_file(a) && is_file(b) {
        return a == b;
    }
    let pa = if is_file(a) { a.to_string() } else { prefix(a) };
    let pb = if is_file(b) { b.to_string() } else { prefix(b) };
    pa.starts_with(&pb) || pb.starts_with(&pa)
}

pub fn subtasks_overlap(a: &Subtask, b: &Subtask) -> bool {
    a.files.iter().any(|x| b.files.iter().any(|y| globs_overlap(x, y)))
}

/// Fan out only when file ownership does not overlap: returns waves (run in parallel inside a wave, waves in order).
/// Overlapping or dependent subtasks land in separate waves. A malformed plan (unknown deps, cycles, no files,
/// duplicate ids) falls back to fully serial execution.
pub fn plan_waves(subtasks: &[Subtask]) -> Vec<Vec<Subtask>> {
    let serial = || subtasks.iter().map(|s| vec![s.clone()]).collect::<Vec<_>>();
    let ids: HashSet<&String> = subtasks.iter().map(|s| &s.id).collect();
    let bad = subtasks.iter().any(|s| s.files.is_empty() || s.deps.iter().any(|d| !ids.contains(d) || d == &s.id)) || ids.len() != subtasks.len();
    if bad {
        return serial();
    }
    let mut done: HashSet<String> = HashSet::new();
    let mut waves: Vec<Vec<Subtask>> = Vec::new();
    let mut left: Vec<Subtask> = subtasks.to_vec();
    while !left.is_empty() {
        let mut wave: Vec<Subtask> = Vec::new();
        for s in &left {
            if !s.deps.iter().all(|d| done.contains(d)) || wave.iter().any(|w| subtasks_overlap(w, s)) {
                continue;
            }
            wave.push(s.clone());
        }
        if wave.is_empty() {
            return serial(); // cycle
        }
        for w in &wave {
            done.insert(w.id.clone());
        }
        left.retain(|s| !wave.iter().any(|w| w.id == s.id));
        waves.push(wave);
    }
    waves
}

#[cfg(test)]
mod tests {
    use super::*;
    fn st(id: &str, files: &[&str], deps: &[&str]) -> Subtask {
        Subtask { id: id.into(), goal: id.into(), files: files.iter().map(|s| s.to_string()).collect(), deps: deps.iter().map(|s| s.to_string()).collect() }
    }
    fn ids(w: Vec<Vec<Subtask>>) -> Vec<Vec<String>> {
        w.into_iter().map(|x| x.into_iter().map(|s| s.id).collect()).collect()
    }

    #[test]
    fn overlap_rules() {
        assert!(globs_overlap("src/**", "src/a/b.ts"));
        assert!(globs_overlap("src/a/**", "src/**"));
        assert!(!globs_overlap("src/a/**", "src/b/**"));
        assert!(!globs_overlap("a.ts", "b.ts"));
        assert!(globs_overlap("a.ts", "a.ts"));
        assert!(!globs_overlap("srcx/**", "src/**"));
    }

    #[test]
    fn waves() {
        assert_eq!(ids(plan_waves(&[st("a", &["api/**"], &[]), st("b", &["web/**"], &[])])), vec![vec!["a", "b"]]);
        assert_eq!(ids(plan_waves(&[st("a", &["src/**"], &[]), st("b", &["src/x.ts"], &[])])), vec![vec!["a"], vec!["b"]]);
        assert_eq!(ids(plan_waves(&[st("a", &["api/**"], &[]), st("b", &["web/**"], &["a"])])), vec![vec!["a"], vec!["b"]]);
        assert_eq!(plan_waves(&[st("a", &["x/**"], &["b"]), st("b", &["y/**"], &["a"])]).iter().map(|w| w.len()).collect::<Vec<_>>(), vec![1, 1]);
        assert_eq!(plan_waves(&[st("a", &[], &[])]).len(), 1);
    }
}
