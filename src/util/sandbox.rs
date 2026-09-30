use super::proc::{Shell, ShellWrap};
use std::path::Path;
use std::sync::Arc;

fn have(bin: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    std::process::Command::new(finder).arg(bin).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

pub fn bwrap_args(cwd: &str, network: bool, c: &Shell) -> Shell {
    let mut a: Vec<String> = ["--ro-bind", "/", "/", "--bind", cwd, cwd, "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp", "--unshare-pid", "--die-with-parent"].iter().map(|s| s.to_string()).collect();
    if !network {
        a.push("--unshare-net".into());
    }
    a.extend(["--chdir".to_string(), cwd.to_string(), c.file.clone()]);
    a.extend(c.args.clone());
    Shell { file: "bwrap".into(), args: a }
}

pub fn seatbelt_profile(cwd: &str, network: bool) -> String {
    format!(
        "(version 1)(allow default)(deny file-write*)(allow file-write* (subpath {cwd:?}) (subpath \"/private/tmp\") (subpath \"/private/var/folders\") (literal \"/dev/null\")){}",
        if network { "" } else { "(deny network*)" }
    )
}

/// OS-level write confinement for shell commands. Linux: bubblewrap; macOS: Seatbelt. Windows: not implemented.
pub fn sandbox_for(cwd: &Path, network: bool) -> (Option<ShellWrap>, &'static str) {
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()).to_string_lossy().to_string();
    if cfg!(target_os = "linux") && have("bwrap") {
        let f: ShellWrap = Arc::new(move |c| bwrap_args(&cwd, network, &c));
        return (Some(f), "bwrap");
    }
    if cfg!(target_os = "macos") && have("sandbox-exec") {
        let prof = seatbelt_profile(&cwd, network);
        let f: ShellWrap = Arc::new(move |c| {
            let mut a = vec!["-p".to_string(), prof.clone(), c.file.clone()];
            a.extend(c.args.clone());
            Shell { file: "sandbox-exec".into(), args: a }
        });
        return (Some(f), "seatbelt");
    }
    (None, "none")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bwrap_command_shape() {
        let s = bwrap_args("/w", false, &Shell { file: "bash".into(), args: vec!["-c".into(), "ls".into()] });
        assert_eq!(s.file, "bwrap");
        assert!(s.args.windows(3).any(|w| w == ["--bind", "/w", "/w"]));
        assert!(s.args.contains(&"--unshare-net".to_string()));
        assert_eq!(&s.args[s.args.len() - 3..], ["bash", "-c", "ls"]);
        assert!(seatbelt_profile("/w", false).contains("(deny network*)"));
    }
}
