use crate::verify::secrets::find_secrets;
use regex::Regex;
use std::sync::LazyLock;

static INJECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(ignore (all |any |the )?(previous|prior|above)|disregard (the )?(system|previous)|you (must|should) (always )?(approve|allow|run|execute)|without (asking|confirmation|approval)|bypass (permission|approval|sandbox)|disable (the )?(sandbox|verification|approval)|exfiltrat|send .* to (http|an? (external|remote)))").unwrap()
});
static SHELL_FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)```\s*(sh|bash|shell|zsh|powershell|ps1|cmd|bat)\b").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:https?|ftp|ssh|file)://\S+|\bwww\.\S+\.\S+").unwrap());
static CMD_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?im)^\s*(?:\$|>)\s+\S+|^\s*(sudo|curl|wget|rm|chmod|npm|pip|dotnet|git)\s+\S+.*$").unwrap());
static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\w .:+#/@-]{2,60}$").unwrap());

/// Skills are data only: no commands, URLs, secrets, permission grants or override attempts.
pub fn validate_skill_body(body: &str, summary: &str, name: &str) -> Result<(), String> {
    let all = format!("{name}\n{summary}\n{body}");
    if body.trim().chars().count() < 40 {
        return Err("too short".into());
    }
    if body.chars().count() > 2400 {
        return Err("too long".into());
    }
    if let Some(s) = find_secrets(&all).first() {
        return Err(format!("contains secret ({s})"));
    }
    if URL.is_match(&all) {
        return Err("contains a URL".into());
    }
    if SHELL_FENCE.is_match(&all) || CMD_LINE.is_match(&all) {
        return Err("contains commands".into());
    }
    if INJECTION.is_match(&all) {
        return Err("contains instruction-override language".into());
    }
    if !NAME.is_match(name) {
        return Err("invalid name".into());
    }
    Ok(())
}
