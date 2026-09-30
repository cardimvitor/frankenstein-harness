use super::frontmatter::parse_frontmatter;
use crate::util::proc::clip;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct UserSkill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: String,
}

#[derive(Clone, Debug, Default)]
pub struct UserConfig {
    pub rules: String,
    pub skills: Vec<UserSkill>,
}

const SKILL_DIRS: [&str; 4] = [".fh/skills", ".qwen/skills", ".claude/skills", ".agents/skills"];
const RULE_FILES: [&str; 3] = ["AGENTS.md", "FRANKENSTEIN.md", ".fh/rules.md"];

/// Visible, editable user files: AGENTS.md rules and SKILL.md skills. These override internal skills.
pub fn load_user_config(cwd: &Path) -> UserConfig {
    let mut rules: Vec<String> = Vec::new();
    for f in RULE_FILES {
        if let Ok(t) = std::fs::read_to_string(cwd.join(f)) {
            rules.push(format!("# {f}\n{}", t.trim()));
        }
    }
    let mut skills = Vec::new();
    for d in SKILL_DIRS {
        let Ok(rd) = std::fs::read_dir(cwd.join(d)) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path().join("SKILL.md");
            if !e.path().is_dir() || !p.exists() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let (meta, body) = parse_frontmatter(&text);
            let dir_name = e.file_name().to_string_lossy().to_string();
            skills.push(UserSkill {
                name: meta.get("name").cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| dir_name.clone()),
                description: meta.get("description").or_else(|| meta.get("summary")).cloned().unwrap_or_default(),
                body,
                path: format!("{d}/{dir_name}/SKILL.md"),
            });
        }
    }
    UserConfig { rules: clip(&rules.join("\n\n"), 8000), skills }
}
