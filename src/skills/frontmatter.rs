use std::collections::HashMap;

/// Minimal `key: value` frontmatter (enough for SKILL.md). Returns (meta, body).
pub fn parse_frontmatter(text: &str) -> (HashMap<String, String>, String) {
    let t = text.replace("\r\n", "\n");
    let Some(rest) = t.strip_prefix("---\n") else { return (HashMap::new(), text.to_string()) };
    let Some(end) = rest.find("\n---\n").or_else(|| rest.strip_suffix("\n---").map(|r| r.len())) else { return (HashMap::new(), text.to_string()) };
    let head = &rest[..end];
    let body = rest.get(end + 5..).unwrap_or("").trim().to_string();
    let mut meta = HashMap::new();
    for line in head.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            if !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                meta.insert(k.to_string(), v.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string());
            }
        }
    }
    (meta, body)
}

/// "8,9,10" or "17+" -> predicate over an optional version. Empty spec matches anything.
pub fn version_matches(spec: Option<&str>, v: Option<&str>) -> bool {
    let Some(spec) = spec.filter(|s| !s.trim().is_empty()) else { return true };
    let Some(v) = v else { return false };
    let Ok(n) = v.parse::<f64>() else { return false };
    spec.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).any(|p| match p.strip_suffix('+') {
        Some(min) => min.parse::<f64>().map(|m| n >= m).unwrap_or(false),
        None => p.parse::<f64>().map(|m| m == n).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frontmatter_and_versions() {
        let (m, b) = parse_frontmatter("---\nname: x\nscope: stack\nkeywords: a b c\n---\nBody here\n- item");
        assert_eq!(m["name"], "x");
        assert_eq!(m["keywords"], "a b c");
        assert_eq!(b, "Body here\n- item");
        assert!(version_matches(Some("8,9,10"), Some("9")));
        assert!(!version_matches(Some("8,9,10"), Some("6")));
        assert!(version_matches(Some("17+"), Some("18")));
        assert!(!version_matches(Some("17+"), Some("15")));
        assert!(version_matches(None, None));
    }
}
