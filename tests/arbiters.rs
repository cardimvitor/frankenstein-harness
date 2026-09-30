use fh::session::checkpoint::Checkpoints;
use fh::verify::checks::Status;
use fh::verify::format::format_check;
use std::path::Path;
use std::process::Command;

fn sh(cwd: &Path, c: &str) {
    assert!(Command::new("sh").arg("-c").arg(c).current_dir(cwd).status().unwrap().success(), "{c}");
}

fn have(bin: &str) -> bool {
    Command::new("which").arg(bin).output().map(|o| o.status.success()).unwrap_or(false)
}

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (n, c) in files {
        let p = d.path().join(n);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, c).unwrap();
    }
    sh(d.path(), "git init -q && git config user.email a@b && git config user.name t && git add -A && git commit -qm init");
    d
}

#[tokio::test]
async fn format_check_blames_only_files_that_were_clean() {
    let d = repo(&[("Cargo.toml", "[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"2021\"\n"), ("src/clean.rs", "fn a() {}\n"), ("src/messy.rs", "fn   b( ){ }\n")]);
    let cp = Checkpoints::new(d.path());
    let base = cp.create("base").await.unwrap();
    // task: breaks formatting of the clean file, touches the already-messy file, adds a clean new file
    std::fs::write(d.path().join("src/clean.rs"), "fn   a( ){ }\n").unwrap();
    std::fs::write(d.path().join("src/messy.rs"), "fn   b( ){ let x=1; }\n").unwrap();
    std::fs::write(d.path().join("src/new.rs"), "fn c() {}\n").unwrap();
    let changed: Vec<String> = ["src/clean.rs", "src/messy.rs", "src/new.rs"].iter().map(|s| s.to_string()).collect();
    let r = format_check(d.path(), &cp, &base, &changed).await.expect("rustfmt applies");
    assert_eq!(r.status, Status::Fail);
    assert!(r.detail.contains("src/clean.rs"), "{}", r.detail);
    assert!(!r.detail.contains("messy.rs") && !r.detail.contains("new.rs"), "{}", r.detail);
    assert!(!d.path().join(".fh/basefmt").exists());
    // fixing the clean file passes
    std::fs::write(d.path().join("src/clean.rs"), "fn a() {}\n").unwrap();
    let r = format_check(d.path(), &cp, &base, &["src/clean.rs".to_string(), "src/new.rs".to_string()]).await.unwrap();
    assert_eq!(r.status, Status::Pass, "{}", r.detail);
}

#[tokio::test]
async fn format_check_gofmt_and_not_applicable() {
    if !have("gofmt") {
        eprintln!("skipped: gofmt not installed");
        return;
    }
    let d = repo(&[("go.mod", "module x\n\ngo 1.21\n"), ("a.go", "package x\n\nfunc A() {}\n"), ("README.md", "hi\n")]);
    let cp = Checkpoints::new(d.path());
    let base = cp.create("base").await.unwrap();
    std::fs::write(d.path().join("a.go"), "package x\nfunc A(){\n}\n").unwrap();
    let r = format_check(d.path(), &cp, &base, &["a.go".to_string()]).await.unwrap();
    assert_eq!(r.status, Status::Fail);
    // markdown only: no configured formatter -> no check at all
    assert!(format_check(d.path(), &cp, &base, &["README.md".to_string()]).await.is_none());
}

#[tokio::test]
async fn format_check_needs_config_for_ruff() {
    if !have("ruff") {
        eprintln!("skipped: ruff not installed");
        return;
    }
    let d = repo(&[("a.py", "x = 1\n")]);
    let cp = Checkpoints::new(d.path());
    let base = cp.create("base").await.unwrap();
    std::fs::write(d.path().join("a.py"), "x   =  1\n").unwrap();
    assert!(format_check(d.path(), &cp, &base, &["a.py".to_string()]).await.is_none(), "no ruff config, so no check");
    std::fs::write(d.path().join("ruff.toml"), "line-length = 88\n").unwrap();
    let r = format_check(d.path(), &cp, &base, &["a.py".to_string()]).await.unwrap();
    assert_eq!(r.status, Status::Fail);
}

mod lsp {
    use super::*;
    use fh::verify::lsp::{diagnostics_check, LspSpec};

    fn spec(pull: bool) -> LspSpec {
        let mut args = vec![format!("{}/tests/fixtures/mock_lsp.py", env!("CARGO_MANIFEST_DIR"))];
        if pull {
            args.push("--pull".into());
        }
        LspSpec { name: "mock".into(), cmd: "python3".into(), args, exts: vec!["mock".into()], language_ids: Default::default(), timeout_ms: 5000 }
    }

    async fn scenario(pull: bool) {
        let d = repo(&[("a.mock", "ok\nERROR old problem\nok\n"), ("b.mock", "fine\n")]);
        let cp = Checkpoints::new(d.path());
        let base = cp.create("base").await.unwrap();
        let specs = [spec(pull)];
        // pre-existing error shifts down two lines: not new. A warning is ignored.
        std::fs::write(d.path().join("a.mock"), "new line\nnew line 2\nok\nERROR old problem\nWARN just a warning\n").unwrap();
        let r = diagnostics_check(d.path(), &cp, &base, &["a.mock".into()], &specs).await.expect("server applies");
        assert_eq!(r.status, Status::Pass, "{}", r.detail);
        // a new error in an existing file and a brand-new file with an error are both reported
        std::fs::write(d.path().join("a.mock"), "ok\nERROR old problem\nERROR brand new\n").unwrap();
        std::fs::write(d.path().join("c.mock"), "ERROR in new file\n").unwrap();
        let r = diagnostics_check(d.path(), &cp, &base, &["a.mock".into(), "c.mock".into()], &specs).await.unwrap();
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("a.mock:3") && r.detail.contains("brand new") && r.detail.contains("c.mock:1"), "{}", r.detail);
        assert!(!r.detail.contains("old problem"), "{}", r.detail);
        // no file with a matching extension: no check
        assert!(diagnostics_check(d.path(), &cp, &base, &["x.txt".into()], &specs).await.is_none());
    }

    #[tokio::test]
    async fn only_new_errors_count_push_diagnostics() {
        scenario(false).await;
    }

    #[tokio::test]
    async fn only_new_errors_count_pull_diagnostics() {
        scenario(true).await;
    }

    #[tokio::test]
    async fn missing_server_is_skipped_not_failed() {
        let d = repo(&[("a.mock", "ok\n")]);
        let cp = Checkpoints::new(d.path());
        let base = cp.create("base").await.unwrap();
        let mut s = spec(false);
        s.cmd = "definitely-not-installed-lsp".into();
        std::fs::write(d.path().join("a.mock"), "ok2\n").unwrap();
        let r = diagnostics_check(d.path(), &cp, &base, &["a.mock".into()], &[s]).await.unwrap();
        assert_eq!(r.status, Status::Skipped);
    }
}

mod real_pyright {
    use super::*;
    use fh::verify::lsp::load_specs;
    use fh::verify::lsp::diagnostics_check;

    #[tokio::test]
    async fn pyright_catches_a_new_type_error_and_ignores_the_old_one() {
        if Command::new("which").arg("pyright-langserver").output().map(|o| !o.status.success()).unwrap_or(true) {
            eprintln!("skipped: pyright-langserver not installed");
            return;
        }
        let d = repo(&[("m.py", "def old() -> int:\n    return \"pre-existing type error\"\n\n\ndef good(x: int) -> int:\n    return x\n")]);
        let cp = Checkpoints::new(d.path());
        let base = cp.create("base").await.unwrap();
        let specs = load_specs(d.path(), &Default::default());
        assert!(specs.iter().any(|s| s.name == "python"), "python server should be detected");
        // unchanged old error only -> pass
        std::fs::write(d.path().join("m.py"), "# comment\ndef old() -> int:\n    return \"pre-existing type error\"\n\n\ndef good(x: int) -> int:\n    return x\n").unwrap();
        let r = diagnostics_check(d.path(), &cp, &base, &["m.py".into()], &specs).await.unwrap();
        assert_eq!(r.status, Status::Pass, "{}", r.detail);
        // introduce a new one
        std::fs::write(d.path().join("m.py"), "def old() -> int:\n    return \"pre-existing type error\"\n\n\ndef good(x: int) -> int:\n    return x\n\n\ndef bad() -> str:\n    return good(\"not an int\")\n").unwrap();
        let r = diagnostics_check(d.path(), &cp, &base, &["m.py".into()], &specs).await.unwrap();
        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(r.detail.contains("m.py:10") || r.detail.contains("m.py:9"), "{}", r.detail);
        assert!(!r.detail.contains("m.py:2:"), "{}", r.detail);
    }
}

mod real_tsserver {
    use super::*;
    use fh::verify::lsp::{diagnostics_check, load_specs};

    /// Set FH_TEST_TSLS to a directory whose node_modules has typescript and typescript-language-server.
    #[tokio::test]
    async fn typescript_language_server_flags_only_new_errors() {
        let Some(nm) = std::env::var_os("FH_TEST_TSLS").map(|p| std::path::PathBuf::from(p).join("node_modules")).filter(|p| p.join(".bin/typescript-language-server").exists()) else {
            eprintln!("skipped: set FH_TEST_TSLS to enable");
            return;
        };
        let d = repo(&[("tsconfig.json", "{\"compilerOptions\":{\"strict\":true,\"target\":\"es2020\",\"module\":\"commonjs\"},\"include\":[\"*.ts\"]}"), ("a.ts", "export const old: number = \"pre-existing\";\nexport function ok(x: number): number { return x; }\n")]);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&nm, d.path().join("node_modules")).unwrap();
        let cp = Checkpoints::new(d.path());
        let base = cp.create("base").await.unwrap();
        let specs = load_specs(d.path(), &Default::default());
        assert!(specs.iter().any(|s| s.name == "typescript"));
        std::fs::write(d.path().join("a.ts"), "// note\nexport const old: number = \"pre-existing\";\nexport function ok(x: number): number { return x; }\n").unwrap();
        let r = diagnostics_check(d.path(), &cp, &base, &["a.ts".into()], &specs).await.unwrap();
        assert_eq!(r.status, Status::Pass, "{}", r.detail);
        std::fs::write(d.path().join("a.ts"), "export const old: number = \"pre-existing\";\nexport function ok(x: number): number { return x; }\nexport const bad: string = ok(\"nope\");\n").unwrap();
        let r = diagnostics_check(d.path(), &cp, &base, &["a.ts".into()], &specs).await.unwrap();
        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(r.detail.contains("a.ts:3"), "{}", r.detail);
        assert!(!r.detail.contains("a.ts:1:"), "{}", r.detail);
    }
}

mod real_rust_analyzer {
    use super::*;
    use fh::verify::lsp::{diagnostics_check, load_specs};

    #[tokio::test]
    async fn rust_analyzer_flags_only_new_errors() {
        if !Command::new("rust-analyzer").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) || !have("cargo") {
            eprintln!("skipped: rust-analyzer not installed");
            return;
        }
        let d = repo(&[("Cargo.toml", "[package]\nname=\"t\"\nversion=\"0.1.0\"\nedition=\"2021\"\n"), ("src/lib.rs", "pub fn old() -> i32 { \"pre-existing\" }\npub fn ok(x: i32) -> i32 { x }\n")]);
        let cp = Checkpoints::new(d.path());
        let base = cp.create("base").await.unwrap();
        let specs = load_specs(d.path(), &Default::default());
        assert!(specs.iter().any(|s| s.name == "rust"));
        std::fs::write(d.path().join("src/lib.rs"), "// note\npub fn old() -> i32 { \"pre-existing\" }\npub fn ok(x: i32) -> i32 { x }\n").unwrap();
        let r = diagnostics_check(d.path(), &cp, &base, &["src/lib.rs".into()], &specs).await.unwrap();
        assert_eq!(r.status, Status::Pass, "{}", r.detail);
        std::fs::write(d.path().join("src/lib.rs"), "pub fn old() -> i32 { \"pre-existing\" }\npub fn ok(x: i32) -> i32 { x }\npub fn bad() -> String { ok(\"nope\") }\n").unwrap();
        let r = diagnostics_check(d.path(), &cp, &base, &["src/lib.rs".into()], &specs).await.unwrap();
        assert_eq!(r.status, Status::Fail, "{}", r.detail);
        assert!(r.detail.contains("src/lib.rs:3"), "{}", r.detail);
    }
}
