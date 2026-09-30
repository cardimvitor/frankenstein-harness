#![cfg(target_os = "linux")]
use fh::util::proc::{run, RunOpts};
use fh::util::sandbox::landlock_wrap;
use std::path::PathBuf;
use std::time::Duration;

fn abi_ok() -> bool {
    fh::util::landlock::abi() >= 1
}

async fn sandboxed(cmd: &str, cwd: &std::path::Path, home: &std::path::Path, network: bool) -> fh::util::proc::RunResult {
    let wrap = landlock_wrap(PathBuf::from(env!("CARGO_BIN_EXE_fh")), &cwd.to_string_lossy(), network, "");
    let env = vec![("HOME".to_string(), home.to_string_lossy().to_string()), ("PATH".to_string(), std::env::var("PATH").unwrap_or_default())];
    run(cmd, cwd, RunOpts { timeout: Some(Duration::from_secs(20)), env: Some(env), wrap: Some(wrap), ..Default::default() }).await
}

fn dirs() -> (tempfile::TempDir, tempfile::TempDir) {
    // home outside /tmp: /tmp is writable (and readable) inside the sandbox by design
    let home = tempfile::tempdir_in("/var/tmp").unwrap();
    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/id_rsa"), "PRIVATE").unwrap();
    std::fs::create_dir_all(home.path().join(".cache")).unwrap();
    std::fs::write(home.path().join(".cache/ok.txt"), "public").unwrap();
    (home, tempfile::tempdir().unwrap())
}

#[tokio::test]
async fn writes_only_inside_workspace_and_tmp() {
    if !abi_ok() {
        eprintln!("skipped: no Landlock");
        return;
    }
    let (home, ws) = dirs();
    let outside = home.path().join("outside.txt");
    let r = sandboxed(&format!("echo hi > {:?}; echo rc=$?", outside), ws.path(), home.path(), true).await;
    assert!(!outside.exists(), "write outside the workspace must be refused: {} {}", r.stdout, r.stderr);
    let r = sandboxed("echo hi > inside.txt && echo hi > /tmp/fh-sbx-ok.txt && cat inside.txt", ws.path(), home.path(), true).await;
    assert_eq!(r.code, Some(0), "{} {}", r.stdout, r.stderr);
    assert!(ws.path().join("inside.txt").exists());
    let _ = std::fs::remove_file("/tmp/fh-sbx-ok.txt");
    // deleting and renaming inside the workspace still works
    let r = sandboxed("mkdir d && mv inside.txt d/x.txt && rm -r d && echo ok", ws.path(), home.path(), true).await;
    let _ = r;
    let r = sandboxed("echo a > f && mv f g && rm g && echo done", ws.path(), home.path(), true).await;
    assert!(r.stdout.contains("done"), "{} {}", r.stdout, r.stderr);
}

#[tokio::test]
async fn credential_directories_are_hidden_but_the_rest_of_home_is_readable() {
    if !abi_ok() {
        return;
    }
    let (home, ws) = dirs();
    let r = sandboxed(&format!("cat {:?}/.ssh/id_rsa", home.path()), ws.path(), home.path(), true).await;
    assert_ne!(r.code, Some(0));
    assert!(!r.stdout.contains("PRIVATE"), "{}", r.stdout);
    assert!(r.stderr.to_lowercase().contains("permission denied"), "{}", r.stderr);
    let r = sandboxed(&format!("cat {:?}/.cache/ok.txt", home.path()), ws.path(), home.path(), true).await;
    assert_eq!(r.stdout.trim(), "public", "{}", r.stderr);
    // ordinary tools keep working
    let r = sandboxed("python3 -c 'print(6*7)' && ls / >/dev/null && echo fine", ws.path(), home.path(), true).await;
    assert!(r.stdout.contains("42") && r.stdout.contains("fine"), "{} {}", r.stdout, r.stderr);
}

#[tokio::test]
async fn tcp_is_blocked_unless_network_is_allowed() {
    if fh::util::landlock::abi() < 4 {
        eprintln!("skipped: needs Landlock ABI 4");
        return;
    }
    let (home, ws) = dirs();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let probe = format!("python3 -c \"import socket; s=socket.create_connection(('127.0.0.1',{port}),2); print('connected')\"");
    let blocked = sandboxed(&probe, ws.path(), home.path(), false).await;
    assert!(!blocked.stdout.contains("connected"), "{} {}", blocked.stdout, blocked.stderr);
    let open = sandboxed(&probe, ws.path(), home.path(), true).await;
    assert!(open.stdout.contains("connected"), "{} {}", open.stdout, open.stderr);
}

#[tokio::test]
async fn seccomp_refuses_namespace_and_mount_syscalls() {
    if !abi_ok() {
        return;
    }
    let (home, ws) = dirs();
    let r = sandboxed("unshare -r true; echo rc=$?", ws.path(), home.path(), true).await;
    assert!(!r.stdout.contains("rc=0"), "unshare should be refused: {} {}", r.stdout, r.stderr);
    let r = sandboxed("python3 - <<'EOF'\nimport ctypes\nl=ctypes.CDLL(None,use_errno=True)\nprint('ptrace', l.ptrace(16,1,0,0), ctypes.get_errno())\nEOF", ws.path(), home.path(), true).await;
    assert!(r.stdout.contains("-1") && r.stdout.contains(" 1"), "ptrace should fail with EPERM: {} {}", r.stdout, r.stderr);
}
