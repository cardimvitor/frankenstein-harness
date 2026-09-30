use regex::Regex;
use std::sync::LazyLock;

/// Secret patterns shared by the verifier (added lines) and the skill store (writes).
static SECRET_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    let p: [(&str, &str); 8] = [
        ("private key", r"-----BEGIN (?:RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----"),
        ("aws access key", r"\bAKIA[0-9A-Z]{16}\b"),
        ("github token", r"\b(?:ghp|gho|ghu|ghs|ghr|github_pat)_[A-Za-z0-9_]{20,}\b"),
        ("slack token", r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b"),
        ("generic api key", r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd)\b["']?\s*[:=]\s*["'][A-Za-z0-9_\-+/=]{16,}["']"#),
        ("bearer token", r"\bBearer\s+[A-Za-z0-9._\-]{24,}\b"),
        ("anthropic/openai key", r"\bsk-(?:ant-)?[A-Za-z0-9_-]{20,}\b"),
        ("jwt", r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b"),
    ];
    p.iter().map(|(n, r)| (*n, Regex::new(r).unwrap())).collect()
});

pub fn find_secrets(text: &str) -> Vec<&'static str> {
    SECRET_PATTERNS.iter().filter(|(_, re)| re.is_match(text)).map(|(n, _)| *n).collect()
}
