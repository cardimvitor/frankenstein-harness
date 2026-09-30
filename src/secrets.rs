//! OS keychain access for the vLLM API key (Linux: libsecret `secret-tool`; macOS: `security`). Never stored in config.
use std::io::Write;
use std::process::{Command, Stdio};

pub const SERVICE: &str = "frankenstein-harness";

/// (program, args) used to read a secret; exposed for tests.
pub fn lookup_command(account: &str) -> Option<(String, Vec<String>)> {
    if cfg!(target_os = "linux") {
        Some(("secret-tool".into(), vec!["lookup".into(), "service".into(), SERVICE.into(), "account".into(), account.into()]))
    } else if cfg!(target_os = "macos") {
        Some(("security".into(), vec!["find-generic-password".into(), "-s".into(), SERVICE.into(), "-a".into(), account.into(), "-w".into()]))
    } else {
        None
    }
}

pub fn get(account: &str) -> Option<String> {
    let (prog, args) = lookup_command(account)?;
    let out = Command::new(prog).args(args).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn set(account: &str, value: &str) -> Result<(), String> {
    if cfg!(target_os = "linux") {
        let mut c = Command::new("secret-tool").args(["store", "--label=Frankenstein Harness API key", "service", SERVICE, "account", account]).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("secret-tool not available ({e}); install libsecret-tools or use the environment variable"))?;
        c.stdin.take().unwrap().write_all(value.as_bytes()).map_err(|e| e.to_string())?;
        let o = c.wait_with_output().map_err(|e| e.to_string())?;
        if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
    } else if cfg!(target_os = "macos") {
        // note: `security` takes the secret as an argument, visible to other local users in the process list for an instant
        let o = Command::new("security").args(["add-generic-password", "-U", "-s", SERVICE, "-a", account, "-w", value]).output().map_err(|e| e.to_string())?;
        if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
    } else {
        Err("keychain storage is not implemented on this OS; use the environment variable".into())
    }
}

pub fn clear(account: &str) -> Result<(), String> {
    let o = if cfg!(target_os = "linux") {
        Command::new("secret-tool").args(["clear", "service", SERVICE, "account", account]).output()
    } else if cfg!(target_os = "macos") {
        Command::new("security").args(["delete-generic-password", "-s", SERVICE, "-a", account]).output()
    } else {
        return Err("keychain storage is not implemented on this OS".into());
    };
    match o {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookup_command_shape() {
        if let Some((_, a)) = lookup_command("FH_API_KEY") {
            assert!(a.contains(&SERVICE.to_string()) && a.contains(&"FH_API_KEY".to_string()));
        }
    }
}
