//! Exercise the actual executable with one file and an isolated SQLx database.
use sqlx::{ConnectOptions, PgPool};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn command(path: &Path, verb: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pab-server"));
    command
        .args([verb, "--config"])
        .arg(path)
        // Legacy environment must not override the file or supply secrets.
        .env("PAB_DATABASE_URL", "invalid-old-database")
        .env("PAB_RELAY_DEFAULT_USER_MBPS", "999")
        .env("PAB_WEB_ORIGIN", "https://wrong.example")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}
async fn settings(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT policy_revision FROM server_settings")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn file_drives_init_restart_origin_and_assets(pool: PgPool) {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("pab-server.toml");
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    std::fs::write(root.path().join("cert.pem"), cert.cert.pem()).unwrap();
    std::fs::write(
        root.path().join("key.pem"),
        cert.signing_key.serialize_pem(),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("web")).unwrap();
    std::fs::write(root.path().join("web/index.html"), "configured-web-assets").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let text = format!(
        r#"
listen = "127.0.0.1:{port}"
[database]
url = {}
[tls]
cert = "cert.pem"
key = "key.pem"
[web]
origin = "https://public.example"
assets = "web"
[relay]
control_secret = "isolated-test-control-secret-at-least-32-bytes"
[log]
directory = "logs"
"#,
        serde_json::to_string(pool.connect_options().to_url_lossy().as_str()).unwrap()
    );
    std::fs::write(&config, &text).unwrap();
    assert!(command(&config, "init").status().unwrap().success());
    assert_eq!(settings(&pool).await, 1);
    sqlx::query("UPDATE server_settings SET policy_revision=2")
        .execute(&pool)
        .await
        .unwrap();
    // Invalid config fails before modifying an existing database.
    std::fs::write(
        &config,
        text.replace("[relay]", "[relay]\ndefault_user_mbps = 10"),
    )
    .unwrap();
    assert!(!command(&config, "init").status().unwrap().success());
    assert_eq!(settings(&pool).await, 2);
    std::fs::write(&config, &text).unwrap();

    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .add_root_certificate(reqwest::Certificate::from_pem(cert.cert.pem().as_bytes()).unwrap())
        .resolve("localhost", ([127, 0, 0, 1], port).into())
        .build()
        .unwrap();
    let origin = format!("https://localhost:{port}");
    for _ in 0..2 {
        let mut service = Running(command(&config, "serve").spawn().unwrap());
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(
                    service.0.try_wait().unwrap().is_none(),
                    "isolated server exited"
                );
                if http
                    .get(format!("{origin}/health"))
                    .send()
                    .await
                    .is_ok_and(|r| r.status() == 204)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("isolated HTTPS startup timed out");
        assert_eq!(settings(&pool).await, 2);
        let assets = http
            .get(&origin)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(assets, "configured-web-assets");
        for (request_origin, rejected) in [
            ("https://public.example", false),
            ("https://wrong.example", true),
        ] {
            let response = http
                .post(format!("{origin}/api/web/session"))
                .header("Origin", request_origin)
                .json(&serde_json::json!({}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status() == 403, rejected);
        }
        drop(service);
    }
    assert!(root.path().join("logs/server.log").is_file());
}
