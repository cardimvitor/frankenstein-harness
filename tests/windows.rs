//! Windows-only checks that need a real Windows host (run by CI on windows-latest): the launch requirement of building
//! and testing a classic .NET Framework 4.8 solution through the verification pipeline, job-object process control,
//! PowerShell quoting, and Credential Manager storage.
#![cfg(windows)]
use fh::fingerprint::fingerprint;
use fh::verify::checks::{run_commands, Status};
use std::path::Path;
use std::time::{Duration, Instant};

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap().flatten() {
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), to).unwrap();
        }
    }
}

fn vswhere_available() -> bool {
    let pf = std::env::var("ProgramFiles(x86)").unwrap_or_default();
    Path::new(&pf).join("Microsoft Visual Studio/Installer/vswhere.exe").exists()
}

#[tokio::test]
async fn classic_dotnet_framework_solution_builds_and_tests_through_msbuild_and_vstest() {
    if !vswhere_available() {
        eprintln!("skipped: Visual Studio / Build Tools (vswhere) not installed");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/netfx"), d.path());
    let fp = fingerprint(d.path());
    assert!(fp.stacks.iter().any(|s| s.id == "dotnet-framework" && s.version.as_deref() == Some("48")), "{:?}", fp.stacks);
    assert_eq!(fp.verify.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), vec!["msbuild", "vstest"]);
    let res = run_commands(d.path(), &fp.verify, None, Duration::from_secs(900)).await;
    for r in &res {
        eprintln!("[{}] {} ({} ms)\n{}", r.name, r.status.as_str(), r.ms, r.detail);
    }
    assert_eq!(res[0].status, Status::Pass, "msbuild: {}", res[0].detail);
    assert_eq!(res[1].status, Status::Pass, "vstest: {}", res[1].detail);
    // a wrong result makes the tests fail (and the build stays green)
    std::fs::write(d.path().join("Lib/Calc.cs"), "namespace Lib { public static class Calc { public static int Add(int a, int b) { return a - b; } } }\n").unwrap();
    let res = run_commands(d.path(), &fp.verify, None, Duration::from_secs(900)).await;
    assert_eq!(res[0].status, Status::Pass, "{}", res[0].detail);
    assert_eq!(res[1].status, Status::Fail, "{}", res[1].detail);
    // a compile error fails the build and skips the tests
    std::fs::write(d.path().join("Lib/Calc.cs"), "namespace Lib { public static class Calc { public static int Add(int a, int b) { return a +; } } }\n").unwrap();
    let res = run_commands(d.path(), &fp.verify, None, Duration::from_secs(900)).await;
    assert_eq!(res[0].status, Status::Fail);
    assert_eq!(res[1].status, Status::Skipped);
}

#[tokio::test]
async fn timeout_kills_the_whole_process_tree_quickly() {
    use fh::util::proc::{run, RunOpts};
    let d = tempfile::tempdir().unwrap();
    let t0 = Instant::now();
    // a shell that starts a long-running child of its own
    let r = run("Start-Process -FilePath ping -ArgumentList '-n','60','127.0.0.1' -NoNewWindow -Wait", d.path(), RunOpts { timeout: Some(Duration::from_secs(2)), ..Default::default() }).await;
    assert!(r.timed_out, "{r:?}");
    assert!(t0.elapsed() < Duration::from_secs(15), "took {:?}", t0.elapsed());
}

#[tokio::test]
async fn powershell_runs_commands_stdin_hooks_and_quoted_paths() {
    use fh::util::proc::{run, RunOpts};
    let d = tempfile::tempdir().unwrap();
    let r = run("Write-Output 'hello from powershell'", d.path(), RunOpts::default()).await;
    assert_eq!(r.code, Some(0));
    assert!(r.stdout.contains("hello from powershell"), "{r:?}");
    let r = run("$i = [Console]::In.ReadToEnd(); Write-Output \"got:$i\"", d.path(), RunOpts { stdin: Some("payload".into()), ..Default::default() }).await;
    assert!(r.stdout.contains("got:payload"), "{r:?}");
    let r = run("exit 3", d.path(), RunOpts::default()).await;
    assert_eq!(r.code, Some(3));
}

#[tokio::test]
async fn edit_tool_syntax_feedback_works_with_powershell_quoting() {
    use fh::tools::fs::WriteNew;
    use fh::tools::{Tool, ToolCtx};
    if std::process::Command::new(fh::util::proc::python().split(' ').next().unwrap()).arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("skipped: no Python");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let ctx = ToolCtx::new(d.path());
    let w = WriteNew::new();
    let bad = w.execute(&serde_json::json!({"path": "it's bad.py", "content": "def f(:\n"}), &ctx).await;
    assert!(bad.ok && bad.output.contains("syntax check failed"), "{}", bad.output);
    let good = w.execute(&serde_json::json!({"path": "good.py", "content": "x = 1\n"}), &ctx).await;
    assert!(good.ok && !good.output.contains("syntax"), "{}", good.output);
}

#[test]
fn credential_manager_roundtrip() {
    // PasswordVault needs an interactive-capable session; report what happens instead of hiding it
    let acct = format!("FH_TEST_KEY_{}", std::process::id());
    match fh::secrets::set(&acct, "s3cret-value") {
        Ok(()) => {
            let got = fh::secrets::get(&acct);
            let _ = fh::secrets::clear(&acct);
            eprintln!("credential manager roundtrip: {got:?}");
            assert_eq!(got.as_deref(), Some("s3cret-value"));
        }
        Err(e) => eprintln!("credential manager not usable in this session: {e}"),
    }
}
