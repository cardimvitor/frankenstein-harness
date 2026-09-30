use fh::session::checkpoint::Checkpoints;
use fh::verify::checks::Status;
use fh::verify::format::format_check;
use std::path::Path;
use std::process::Command;

fn sh(cwd: &Path, c: &str) {
    assert!(Command::new("sh").arg("-c").arg(c).current_dir(cwd).status().unwrap().success(), "{c}");
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
    let d = repo(&[("a.py", "x = 1\n")]);
    let cp = Checkpoints::new(d.path());
    let base = cp.create("base").await.unwrap();
    std::fs::write(d.path().join("a.py"), "x   =  1\n").unwrap();
    assert!(format_check(d.path(), &cp, &base, &["a.py".to_string()]).await.is_none(), "no ruff config, so no check");
    std::fs::write(d.path().join("ruff.toml"), "line-length = 88\n").unwrap();
    let r = format_check(d.path(), &cp, &base, &["a.py".to_string()]).await.unwrap();
    assert_eq!(r.status, Status::Fail);
}
