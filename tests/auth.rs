use fh::config::{auth_headers, load_config, Config, Env};
use fh::llm::client::{ChatOptions, LlmClient};
use fh::testkit::{self, Scripted};
use fh::types::Message;
use std::path::Path;
use std::process::{Child, Command, Stdio};

fn base_cfg() -> Config {
    let mut c = load_config(Path::new("/x"), &Env::new()).unwrap();
    c.retries = 2;
    c
}

#[cfg(unix)]
#[tokio::test]
async fn token_command_is_cached_and_refreshed_after_a_401() {
    let d = tempfile::tempdir().unwrap();
    // first call prints a stale token, later calls a good one
    let script = d.path().join("tok.sh");
    std::fs::write(&script, format!("#!/bin/sh\nf={:?}/n\nn=$(cat $f 2>/dev/null || echo 0)\nn=$((n+1))\necho $n > $f\nif [ $n -eq 1 ]; then echo stale-token-aaaaaaaa; else echo good-token-bbbbbbbb; fi\n", d.path())).unwrap();
    let m = testkit::start(0, Some("good-token-bbbbbbbb")).await;
    m.push(Scripted::text("hello"));
    let mut cfg = base_cfg();
    cfg.endpoint = m.url.clone();
    cfg.auth_token_cmd = format!("sh {:?}", script);
    let env = Env::new();
    let h1 = auth_headers(&cfg, &env);
    assert!(h1.iter().any(|(k, v)| k == "Authorization" && v == "Bearer stale-token-aaaaaaaa"), "{h1:?}");
    // cached: same token, the script did not run again
    assert_eq!(auth_headers(&cfg, &env), h1);
    assert_eq!(std::fs::read_to_string(d.path().join("n")).unwrap().trim(), "1");
    // the server rejects the stale token; the client refreshes and retries once
    let llm = LlmClient::new(cfg.clone(), env.clone());
    let r = llm.chat(ChatOptions { messages: vec![Message::user("hi")], ..Default::default() }).await.expect("retry with the refreshed token succeeds");
    assert_eq!(r.content, "hello");
    assert_eq!(llm.stats().retries, 1);
    assert_eq!(std::fs::read_to_string(d.path().join("n")).unwrap().trim(), "2");
    // the token is redacted from error text and logs
    assert!(!fh::config::redact("x good-token-bbbbbbbb y", &env).contains("good-token"));
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn sh(cwd: &Path, c: &str) {
    assert!(Command::new("sh").arg("-c").arg(c).current_dir(cwd).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success(), "{c}");
}

#[cfg(unix)]
#[tokio::test]
async fn mtls_client_certificate_is_presented_and_required() {
    if Command::new("openssl").arg("version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("skipped: openssl not installed");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    sh(p, "openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -subj /CN=test-ca -days 2");
    sh(p, "openssl req -newkey rsa:2048 -nodes -keyout srv.key -out srv.csr -subj /CN=localhost && printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\\n' > san.ext && openssl x509 -req -in srv.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out srv.pem -days 2 -extfile san.ext");
    sh(p, "openssl req -newkey rsa:2048 -nodes -keyout cli.key -out cli.csr -subj /CN=client && printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=clientAuth\n' > cli.ext && openssl x509 -req -in cli.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out cli.pem -days 2 -extfile cli.ext");
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let _srv = Server(
        Command::new("openssl")
            .current_dir(p)
            .args(["s_server", "-accept", &port.to_string(), "-www", "-cert", "srv.pem", "-key", "srv.key", "-CAfile", "ca.pem", "-Verify", "1", "-verify_return_error"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let url = format!("https://127.0.0.1:{port}/");
    let mut cfg = base_cfg();
    cfg.ca_cert = p.join("ca.pem").to_string_lossy().to_string();
    // without a client certificate the handshake is refused
    let anon = fh::http::client(&cfg).get(&url).header("connection", "close").timeout(std::time::Duration::from_secs(5)).send().await;
    assert!(anon.is_err() || !anon.unwrap().status().is_success(), "server must require a client certificate");
    // with certificate and key (separate files) the request succeeds
    cfg.client_cert = p.join("cli.pem").to_string_lossy().to_string();
    cfg.client_key = p.join("cli.key").to_string_lossy().to_string();
    let ok = fh::http::client(&cfg).get(&url).header("connection", "close").timeout(std::time::Duration::from_secs(5)).send().await.expect("mTLS handshake");
    assert!(ok.status().is_success());
    let _ = ok.text().await; // s_server serves one connection at a time
    // a single combined PEM works as well
    let combined = p.join("both.pem");
    std::fs::write(&combined, format!("{}\n{}", std::fs::read_to_string(p.join("cli.pem")).unwrap(), std::fs::read_to_string(p.join("cli.key")).unwrap())).unwrap();
    cfg.client_cert = combined.to_string_lossy().to_string();
    cfg.client_key = String::new();
    assert!(fh::http::client(&cfg).get(&url).header("connection", "close").timeout(std::time::Duration::from_secs(5)).send().await.unwrap().status().is_success());
    // unreadable certificate paths give a clear error
    cfg.client_cert = "/nonexistent.pem".into();
    assert!(fh::http::client_builder(&cfg).err().unwrap().contains("clientCert"));
}
