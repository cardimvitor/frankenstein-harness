//! UI style decision for generated UI code: follow the project's own design system or patterns first,
//! otherwise use the HIG-based default skill.
use crate::tools::fs::list_all;
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesignSignal {
    /// the project has an explicit design system / component library (name)
    Strong(String),
    /// UI code exists but no explicit design system: existing patterns should be followed, the user may prefer the default
    Weak,
    /// no UI code yet: use the HIG-based default
    None,
}

const UI_WORDS: [&str; 26] = [
    "ui", "ux", "screen", "page", "component", "button", "form", "layout", "style", "styling", "css", "theme", "dark mode", "responsive", "modal", "navbar", "sidebar", "dashboard", "frontend", "front-end", "widget", "dialog", "tooltip", "menu", "landing", "template",
];

pub fn is_ui_task(task: &str) -> bool {
    let t = format!(" {} ", task.to_lowercase().replace(|c: char| !c.is_alphanumeric() && c != '-', " "));
    UI_WORDS.iter().any(|w| t.contains(&format!(" {w} ")) || t.contains(&format!(" {w}s ")))
}

const LIBS: [(&str, &str); 14] = [
    ("tailwindcss", "Tailwind CSS"),
    ("@mui/material", "Material UI"),
    ("antd", "Ant Design"),
    ("@angular/material", "Angular Material"),
    ("bootstrap", "Bootstrap"),
    ("@chakra-ui/react", "Chakra UI"),
    ("@mantine/core", "Mantine"),
    ("primeng", "PrimeNG"),
    ("@fluentui/react", "Fluent UI"),
    ("@fluentui/react-components", "Fluent UI"),
    ("@radix-ui/react-dialog", "Radix UI"),
    ("styled-components", "styled-components"),
    ("@emotion/react", "Emotion"),
    ("vuetify", "Vuetify"),
];

pub fn detect_design(cwd: &Path) -> DesignSignal {
    let files = list_all(cwd, 4000);
    if let Ok(t) = std::fs::read_to_string(cwd.join("package.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&t) {
            for (dep, name) in LIBS {
                if ["dependencies", "devDependencies"].iter().any(|k| v.get(k).and_then(|d| d.get(dep)).is_some()) {
                    return DesignSignal::Strong(name.to_string());
                }
            }
        }
    }
    for f in &files {
        let l = f.to_lowercase();
        let base = l.rsplit('/').next().unwrap_or(&l);
        if base.starts_with("tailwind.config") {
            return DesignSignal::Strong("Tailwind CSS".into());
        }
        if l.starts_with(".storybook/") || base.contains(".stories.") {
            return DesignSignal::Strong("the project's Storybook components".into());
        }
        if base.starts_with("design-tokens") || base.starts_with("tokens.") || matches!(base, "design.md" | "styleguide.md" | "design-system.md") || l.starts_with("docs/design") || l.contains("/styleguide") {
            return DesignSignal::Strong("the project's documented design system".into());
        }
    }
    let ui_ext = [".css", ".scss", ".less", ".tsx", ".jsx", ".vue", ".razor", ".cshtml", ".xaml", ".html", ".svelte"];
    if files.iter().any(|f| ui_ext.iter().any(|e| f.to_lowercase().ends_with(e))) { DesignSignal::Weak } else { DesignSignal::None }
}

pub fn design_note(name: &str) -> String {
    format!("- This project already has a UI foundation ({name}).\n- Reuse its existing components, tokens and naming for any new UI.\n- Do not introduce another UI library or a different styling approach.\n- Match spacing, typography and colour usage of neighbouring screens.\n- Keep accessibility (labels, focus, contrast) at least as good as the surrounding code.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_tasks() {
        assert!(is_ui_task("add a dark mode toggle to the settings page"));
        assert!(is_ui_task("Build the login form"));
        assert!(!is_ui_task("fix the off-by-one in sum_range"));
        assert!(!is_ui_task("optimize the database query"));
    }

    #[test]
    fn detection() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(detect_design(d.path()), DesignSignal::None);
        std::fs::write(d.path().join("app.css"), "a{}").unwrap();
        assert_eq!(detect_design(d.path()), DesignSignal::Weak);
        std::fs::write(d.path().join("package.json"), r#"{"dependencies":{"@mui/material":"^6"}}"#).unwrap();
        assert_eq!(detect_design(d.path()), DesignSignal::Strong("Material UI".into()));
    }
}
