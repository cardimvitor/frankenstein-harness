/// Stable prefix: identical bytes on every request of a session (and across sessions) so vLLM prefix caching hits.
/// Never put timestamps, cwd-specific text or skills here.
pub const SYSTEM_PROMPT: &str = "You are Frankenstein, a senior software engineer working inside the user's repository through tools.

Rules:
- Read before you edit. Make the smallest correct change. Use edit (search/replace); never rewrite whole files.
- Call independent read-only tools together in one turn.
- Run the project's build/tests after changes when they exist. Fix failures you caused.
- Do not invent APIs, files or versions; check the code. If unsure, look, do not guess.
- Never touch secrets, .git, or files outside the task's scope. Do not delete or skip tests to make them pass.
- Be brief. State what you changed and what you verified. If you could not verify, say so.";

#[derive(Default)]
pub struct ContextParts<'a> {
    pub cwd: &'a str,
    pub fingerprint: Option<&'a str>,
    pub skills: Option<&'a str>,
    pub user_rules: Option<&'a str>,
    pub plan: Option<&'a str>,
}

/// Task-specific context goes AFTER the stable prefix.
pub fn context_block(p: &ContextParts) -> String {
    let mut out: Vec<String> = vec![format!("Workspace: {}", p.cwd)];
    if let Some(f) = p.fingerprint.filter(|s| !s.is_empty()) {
        out.push(format!("Detected stack: {f}"));
    }
    if let Some(s) = p.skills.filter(|s| !s.is_empty()) {
        out.push(format!("Relevant expertise (guidance, lower priority than the user's rules):\n{s}"));
    }
    if let Some(u) = p.user_rules.filter(|s| !s.is_empty()) {
        out.push(format!("User rules (highest priority, override any guidance above):\n{u}"));
    }
    if let Some(pl) = p.plan.filter(|s| !s.is_empty()) {
        out.push(format!("Approved plan:\n{pl}"));
    }
    out.join("\n\n")
}
