//! User-defined command rules (allow / prompt / forbid by command prefix), so common safe commands stop asking for
//! approval and dangerous ones are refused in every mode. Files: `<config dir>/rules.json` (user) and
//! `<repo>/.fh/rules.json` (project; its `allow` rules only count in a trusted workspace, `prompt` and `forbid`
//! always do, since restricting is safe).
//!
//! ```json
//! {"rules": [
//!   {"pattern": ["cargo", ["test", "check", "build"]], "decision": "allow", "match": ["cargo test --release"], "not_match": ["cargo publish"]},
//!   {"pattern": ["git", "push"], "decision": "prompt", "justification": "pushing is outward-facing"},
//!   {"pattern": ["terraform", "destroy"], "decision": "forbid", "justification": "never from an agent"}
//! ]}
//! ```
//! A `pattern` is an ordered token prefix; a list element means "any of these". `match` / `not_match` are examples
//! checked when the file is loaded (a rule that fails its own examples is dropped and reported).
//! Chained commands (`a && b`, pipes) are split and every part must pass; a command with redirections or
//! substitutions is never auto-allowed. The strictest decision wins: forbid > prompt > allow.
use crate::config::{config_dir, Env};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleDecision {
    Allow,
    Prompt,
    Forbid,
}

#[derive(Clone, Debug)]
struct Rule {
    pattern: Vec<Vec<String>>,
    decision: RuleDecision,
    justification: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Rules {
    rules: Vec<Rule>,
    /// rules dropped at load time and why
    pub problems: Vec<String>,
}

/// Splits into simple commands at `;`, `&&`, `||`, `|` and newlines outside quotes. The bool says whether the command
/// uses constructs whose effect cannot be judged from the tokens (redirection, substitution, subshell, background).
fn split_commands(cmd: &str) -> (Vec<Vec<String>>, bool) {
    let (mut out, mut cur, mut tok): (Vec<Vec<String>>, Vec<String>, String) = (vec![], vec![], String::new());
    let (mut in_s, mut in_d, mut complex, mut has_tok) = (false, false, false, false);
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    let flush_tok = |tok: &mut String, cur: &mut Vec<String>, has: &mut bool| {
        if *has {
            cur.push(std::mem::take(tok));
            *has = false;
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if in_s {
            if c == '\'' { in_s = false } else { tok.push(c) }
        } else if in_d {
            match c {
                '"' => in_d = false,
                '\\' if i + 1 < chars.len() => {
                    i += 1;
                    tok.push(chars[i]);
                }
                '$' | '`' => {
                    complex = true;
                    tok.push(c);
                }
                _ => tok.push(c),
            }
        } else {
            match c {
                '\'' => {
                    in_s = true;
                    has_tok = true;
                }
                '"' => {
                    in_d = true;
                    has_tok = true;
                }
                '\\' if i + 1 < chars.len() => {
                    i += 1;
                    tok.push(chars[i]);
                    has_tok = true;
                }
                ' ' | '\t' => flush_tok(&mut tok, &mut cur, &mut has_tok),
                ';' | '\n' | '|' | '&' => {
                    // `&&` and `||` are separators; a lone `&` (background) is complex
                    if c == '&' && chars.get(i + 1) != Some(&'&') {
                        complex = true;
                    }
                    if (c == '&' || c == '|') && chars.get(i + 1) == Some(&c) {
                        i += 1;
                    }
                    flush_tok(&mut tok, &mut cur, &mut has_tok);
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                '>' | '<' | '`' | '(' | ')' | '{' | '}' => {
                    complex = true;
                    tok.push(c);
                    has_tok = true;
                }
                '$' => {
                    complex = true;
                    tok.push(c);
                    has_tok = true;
                }
                _ => {
                    tok.push(c);
                    has_tok = true;
                }
            }
        }
        i += 1;
    }
    flush_tok(&mut tok, &mut cur, &mut has_tok);
    if !cur.is_empty() {
        out.push(cur);
    }
    (out, complex || in_s || in_d)
}

fn matches(rule: &Rule, tokens: &[String]) -> bool {
    tokens.len() >= rule.pattern.len() && rule.pattern.iter().zip(tokens).all(|(alts, t)| alts.iter().any(|a| a == t))
}

fn parse_pattern(v: &Value) -> Option<Vec<Vec<String>>> {
    v.as_array()?.iter().map(|e| match e {
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(a) => a.iter().map(|x| x.as_str().map(|s| s.to_string())).collect::<Option<Vec<_>>>().filter(|v| !v.is_empty()),
        _ => None,
    }).collect::<Option<Vec<_>>>().filter(|p| !p.is_empty())
}

impl Rules {
    /// `allow_allow`: whether `allow` rules from this source count (false for a project file in an untrusted workspace).
    pub fn from_json(v: &Value, allow_allow: bool, source: &str) -> Rules {
        let mut r = Rules::default();
        for (i, e) in v.get("rules").and_then(|x| x.as_array()).cloned().unwrap_or_default().iter().enumerate() {
            let Some(pattern) = e.get("pattern").and_then(parse_pattern) else {
                r.problems.push(format!("{source} rule {}: invalid pattern", i + 1));
                continue;
            };
            let decision = match e.get("decision").and_then(|d| d.as_str()).unwrap_or("allow") {
                "allow" => RuleDecision::Allow,
                "prompt" => RuleDecision::Prompt,
                "forbid" | "forbidden" => RuleDecision::Forbid,
                other => {
                    r.problems.push(format!("{source} rule {}: unknown decision \"{other}\"", i + 1));
                    continue;
                }
            };
            if decision == RuleDecision::Allow && !allow_allow {
                r.problems.push(format!("{source} rule {}: allow rules from an untrusted project are ignored (run `fh trust`)", i + 1));
                continue;
            }
            let rule = Rule { pattern, decision, justification: e.get("justification").and_then(|j| j.as_str()).map(|s| s.to_string()) };
            // examples are unit tests for the rule
            let ex = |k: &str| -> Vec<String> { e.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect()).unwrap_or_default() };
            let hits = |c: &str| { let (segs, _) = split_commands(c); segs.first().map(|t| matches(&rule, t)).unwrap_or(false) };
            if let Some(bad) = ex("match").iter().find(|c| !hits(c)) {
                r.problems.push(format!("{source} rule {}: its own example \"{bad}\" does not match; rule dropped", i + 1));
                continue;
            }
            if let Some(bad) = ex("not_match").iter().find(|c| hits(c)) {
                r.problems.push(format!("{source} rule {}: \"{bad}\" must not match but does; rule dropped", i + 1));
                continue;
            }
            r.rules.push(rule);
        }
        r
    }

    pub fn load(cwd: &Path, env: &Env) -> Rules {
        let mut all = Rules::default();
        let read = |p: &Path| std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
        if let Some(v) = read(&config_dir(env).join("rules.json")) {
            all.merge(Rules::from_json(&v, true, "user rules"));
        }
        if let Some(v) = read(&cwd.join(".fh").join("rules.json")) {
            all.merge(Rules::from_json(&v, crate::trust::is_trusted(env, cwd), "project rules"));
        }
        all
    }

    fn merge(&mut self, o: Rules) {
        self.rules.extend(o.rules);
        self.problems.extend(o.problems);
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The decision for a shell command, or None when no rule speaks for it (normal approval flow applies).
    pub fn evaluate(&self, command: &str) -> Option<(RuleDecision, Option<String>)> {
        if self.rules.is_empty() {
            return None;
        }
        let (segs, complex) = split_commands(command);
        if segs.is_empty() {
            return None;
        }
        let mut strictest: Option<(RuleDecision, Option<String>)> = None;
        let mut all_allowed = true;
        for seg in &segs {
            let m: Vec<&Rule> = self.rules.iter().filter(|r| matches(r, seg)).collect();
            match m.iter().max_by_key(|r| r.decision) {
                Some(r) => {
                    if strictest.as_ref().map(|s| r.decision > s.0).unwrap_or(true) {
                        strictest = Some((r.decision, r.justification.clone()));
                    }
                    if r.decision != RuleDecision::Allow {
                        all_allowed = false;
                    }
                }
                None => all_allowed = false,
            }
        }
        match strictest {
            Some((RuleDecision::Forbid, j)) => Some((RuleDecision::Forbid, j)),
            Some((RuleDecision::Prompt, j)) => Some((RuleDecision::Prompt, j)),
            // allow only when every part is allowed and nothing in the command hides behavior from the tokens
            Some((RuleDecision::Allow, j)) if all_allowed && !complex => Some((RuleDecision::Allow, j)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules() -> Rules {
        let r = Rules::from_json(
            &json!({"rules": [
                {"pattern": ["cargo", ["test", "check"]], "decision": "allow", "match": ["cargo test --release", "cargo check"], "not_match": ["cargo publish"]},
                {"pattern": ["git", "status"], "decision": "allow"},
                {"pattern": ["git", "push"], "decision": "prompt", "justification": "outward-facing"},
                {"pattern": ["rm", "-rf"], "decision": "forbid", "justification": "no recursive deletes"}
            ]}),
            true,
            "test",
        );
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        r
    }

    #[test]
    fn prefix_alternatives_and_strictest_wins() {
        let r = rules();
        assert_eq!(r.evaluate("cargo test --release -- --nocapture").map(|x| x.0), Some(RuleDecision::Allow));
        assert_eq!(r.evaluate("cargo check").map(|x| x.0), Some(RuleDecision::Allow));
        assert_eq!(r.evaluate("cargo publish"), None);
        assert_eq!(r.evaluate("git push origin main").map(|x| x.0), Some(RuleDecision::Prompt));
        assert_eq!(r.evaluate("rm -rf build").unwrap().1.as_deref(), Some("no recursive deletes"));
        // chained: a forbidden part poisons an allowed one; an unknown part stops auto-allow
        assert_eq!(r.evaluate("git status && rm -rf /tmp/x").map(|x| x.0), Some(RuleDecision::Forbid));
        assert_eq!(r.evaluate("git status; curl http://x").map(|x| x.0), None);
        assert_eq!(r.evaluate("cargo test && git status").map(|x| x.0), Some(RuleDecision::Allow));
        // quoting is honoured: an operator inside quotes is data
        assert_eq!(r.evaluate("git status \"a && b\"").map(|x| x.0), Some(RuleDecision::Allow));
    }

    #[test]
    fn complex_commands_are_never_auto_allowed_but_can_still_be_forbidden() {
        let r = rules();
        for c in ["cargo test > /etc/passwd", "cargo test $(curl evil)", "cargo test `id`", "cargo test &", "cargo test 'unterminated"] {
            assert_eq!(r.evaluate(c), None, "{c}");
        }
        assert_eq!(r.evaluate("echo $(rm -rf /); rm -rf x").map(|x| x.0), Some(RuleDecision::Forbid));
    }

    #[test]
    fn bad_rules_are_dropped_and_untrusted_allow_is_ignored() {
        let r = Rules::from_json(&json!({"rules": [
            {"pattern": ["ls"], "decision": "allow", "match": ["pwd"]},
            {"pattern": ["ls"], "decision": "allow", "not_match": ["ls -la"]},
            {"pattern": [], "decision": "allow"},
            {"pattern": ["x"], "decision": "maybe"},
            {"pattern": ["git", "push"], "decision": "forbid"}
        ]}), true, "t");
        assert_eq!(r.problems.len(), 4, "{:?}", r.problems);
        assert_eq!(r.evaluate("git push").map(|x| x.0), Some(RuleDecision::Forbid));
        let untrusted = Rules::from_json(&json!({"rules": [{"pattern": ["curl"], "decision": "allow"}, {"pattern": ["curl"], "decision": "prompt"}]}), false, "project rules");
        assert_eq!(untrusted.evaluate("curl http://x").map(|x| x.0), Some(RuleDecision::Prompt), "restricting rules still apply");
        assert!(untrusted.problems.iter().any(|p| p.contains("untrusted")));
    }
}
