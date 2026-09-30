//! The install scripts, exercised against a fake release folder. PowerShell tests need `pwsh` (set FH_TEST_PWSH to its
//! path, or have it on PATH); they skip otherwise. On Windows the real `powershell.exe`/`pwsh` is used.
use std::path::{Path, PathBuf};
use std::process::Command;

fn pwsh() -> Option<String> {
    let c = std::env::var("FH_TEST_PWSH").unwrap_or_else(|_| "pwsh".into());
    Command::new(&c).args(["-NoProfile", "-Command", "exit 0"]).output().ok().filter(|o| o.status.success()).map(|_| c)
}

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts").join(name)
}

fn py(code: &str, cwd: &Path) {
    let python = if cfg!(windows) { "python" } else { "python3" };
    assert!(Command::new(python).arg("-c").arg(code).current_dir(cwd).status().unwrap().success());
}

/// A release folder with fh-0.1.0-x86_64-pc-windows-msvc.zip (+ .sha256) whose fh.exe contains `payload`.
fn fake_release(dir: &Path, payload: &str, tamper: bool) {
    py(&format!(
        "import zipfile,hashlib,os\nn='fh-0.1.0-x86_64-pc-windows-msvc'\nz=zipfile.ZipFile(n+'.zip','w')\nz.writestr(n+'/fh.exe','{payload}')\nz.writestr(n+'/README.md','readme')\nz.close()\nh=hashlib.sha256(open(n+'.zip','rb').read()).hexdigest()\nopen(n+'.zip.sha256','w').write(h+'  '+n+'.zip\\n')\nif {}:\n    b=bytearray(open(n+'.zip','rb').read()); b[-5]^=1; open(n+'.zip','wb').write(b)\n",
        if tamper { "True" } else { "False" }
    ), dir);
}

fn run_ps(p: &str, args: &[&str]) -> std::process::Output {
    Command::new(p).args(["-NoProfile", "-NonInteractive", "-File"]).arg(script("install.ps1")).args(args).output().unwrap()
}

#[test]
fn powershell_installer_verifies_installs_upgrades_and_uninstalls() {
    let Some(p) = pwsh() else {
        eprintln!("skipped: pwsh not available");
        return;
    };
    let rel = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let inst = dest.path().join("Programs").join("fh");
    let (src, dir) = (rel.path().to_str().unwrap(), inst.to_str().unwrap());
    fake_release(rel.path(), "v1", false);
    let o = run_ps(&p, &["-Source", src, "-InstallDir", dir, "-NoPath", "-NoVerify"]);
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("Checksum verified"));
    assert_eq!(std::fs::read_to_string(inst.join("fh.exe")).unwrap(), "v1");
    assert!(inst.join("README.md").exists());
    // upgrade in place: the previous binary is kept as fh.exe.old (a running exe cannot be overwritten, only renamed)
    fake_release(rel.path(), "v2", false);
    assert!(run_ps(&p, &["-Source", src, "-InstallDir", dir, "-NoPath", "-NoVerify"]).status.success());
    assert_eq!(std::fs::read_to_string(inst.join("fh.exe")).unwrap(), "v2");
    assert_eq!(std::fs::read_to_string(inst.join("fh.exe.old")).unwrap(), "v1");
    // uninstall removes the folder
    let o = run_ps(&p, &["-Uninstall", "-InstallDir", dir, "-NoPath"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!inst.exists());
}

#[test]
fn powershell_installer_refuses_a_tampered_download_and_installs_nothing() {
    let Some(p) = pwsh() else {
        return;
    };
    let rel = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let inst = dest.path().join("fh");
    fake_release(rel.path(), "v1", true);
    let o = run_ps(&p, &["-Source", rel.path().to_str().unwrap(), "-InstallDir", inst.to_str().unwrap(), "-NoPath", "-NoVerify"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr).to_string() + &String::from_utf8_lossy(&o.stdout);
    assert!(err.contains("Checksum mismatch") && err.contains("Nothing was installed"), "{err}");
    assert!(!inst.join("fh.exe").exists());
}

#[test]
fn powershell_path_helpers_are_idempotent_and_reversible() {
    let Some(p) = pwsh() else {
        return;
    };
    let s = script("install.ps1");
    let run = |expr: &str| {
        let o = Command::new(&p).args(["-NoProfile", "-NonInteractive", "-Command"]).arg(format!(". '{}'; {expr}", s.display())).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let sep = if cfg!(windows) { ";" } else { ":" };
    let (a, b, fh) = ("/a", "/b", "/fh");
    let base = format!("{a}{sep}{b}");
    assert_eq!(run(&format!("Add-PathEntry '{base}' '{fh}'")), format!("{base}{sep}{fh}"));
    // already present (different case, trailing slash): unchanged
    assert_eq!(run(&format!("Add-PathEntry '{base}{sep}{fh}' '/FH/'")), format!("{base}{sep}{fh}"));
    assert_eq!(run(&format!("Add-PathEntry '' '{fh}'")), fh);
    assert_eq!(run(&format!("Remove-PathEntry '{base}{sep}{fh}' '{fh}/'")), base);
}

#[cfg(unix)]
#[test]
fn shell_installer_verifies_and_installs() {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let target = match (os, arch) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        _ => return,
    };
    let rel = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let n = format!("fh-0.1.0-{target}");
    py(&format!("import tarfile,hashlib,io,os\nn='{n}'\nt=tarfile.open(n+'.tar.gz','w:gz')\ndata=b'#!/bin/sh\\necho fake fh\\n'\ni=tarfile.TarInfo(n+'/fh'); i.size=len(data); i.mode=0o755; t.addfile(i,io.BytesIO(data)); t.close()\nh=hashlib.sha256(open(n+'.tar.gz','rb').read()).hexdigest()\nopen(n+'.tar.gz.sha256','w').write(h+'  '+n+'.tar.gz\\n')\n"), rel.path());
    let run = |src: &Path| Command::new("bash").arg(script("install.sh")).env("SOURCE", src).env("INSTALL_DIR", dest.path().join("bin")).output().unwrap();
    let o = run(rel.path());
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("checksum verified"));
    assert!(dest.path().join("bin/fh").exists());
    // tampered archive is rejected
    let other = tempfile::tempdir().unwrap();
    std::fs::copy(rel.path().join(format!("{n}.tar.gz")), other.path().join(format!("{n}.tar.gz"))).unwrap();
    std::fs::write(other.path().join(format!("{n}.tar.gz.sha256")), format!("{}  {n}.tar.gz\n", "0".repeat(64))).unwrap();
    let o = run(other.path());
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("checksum mismatch"));
}
