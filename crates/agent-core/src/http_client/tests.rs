use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::Arc,
    thread,
    time::Instant,
};

fn accept(listener: TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                return stream;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("fixture listener failed: {}", error.kind()),
        }
    }
}

#[test]
fn environment_proxy_child() {
    let Ok(mode) = std::env::var("PAB_TEST_HTTP_PROXY_CHILD") else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .try_init();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        if mode == "bypass" {
            let pem = std::env::var("PAB_TEST_HTTP_CA").unwrap();
            let response = client(Duration::from_secs(3), Some(pem.as_bytes()))
                .unwrap()
                .get(std::env::var("PAB_TEST_HTTP_URL").unwrap())
                .send()
                .await
                .unwrap();
            assert_eq!(response.text().await.unwrap(), "OK");
            return;
        }
        let result = client(Duration::from_secs(3), None)
            .unwrap()
            .get("https://proxy-target.invalid/check?token=secret-query-certificate")
            .bearer_auth("secret-bearer")
            .send()
            .await;
        let error = result.err().expect("fixture must reject the request");
        let diagnostic = failure("proxy.test", &error);
        assert_eq!(
            diagnostic.kind,
            if mode == "http" {
                FailureKind::ProxyAuthentication
            } else {
                FailureKind::Proxy
            }
        );
        println!("{}", diagnostic.account_code());
    });
}

fn proxy_command(mode: &str, proxy: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "http_client::tests::environment_proxy_child",
            "--nocapture",
        ])
        .env("PAB_TEST_HTTP_PROXY_CHILD", mode)
        .env("NO_PROXY", "")
        .env("no_proxy", "");
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    command.env(
        if mode == "http" {
            "HTTPS_PROXY"
        } else {
            "ALL_PROXY"
        },
        proxy,
    );
    command
}

fn proxy_child(mode: &str, proxy: &str) -> std::process::Output {
    proxy_command(mode, proxy).output().unwrap()
}

#[test]
fn https_proxy_authentication_is_reported_without_disclosing_credentials_or_query() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut socket = accept(listener);
        let mut request = vec![];
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        assert!(request.starts_with(b"CONNECT proxy-target.invalid:443 HTTP/1.1\r\n"));
        socket
            .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
    });
    let output = proxy_child(
        "http",
        &format!("http://secret-proxy-user:secret-proxy-password@{address}"),
    );
    server.join().unwrap();
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("account_proxy_authentication_required"));
    assert!(text.contains("proxy_authentication_required"));
    for secret in [
        "secret-proxy-user",
        "secret-proxy-password",
        "secret-query-certificate",
        "secret-bearer",
        "token=",
    ] {
        assert!(
            !text.contains(secret),
            "transport diagnostics disclosed sensitive data"
        );
    }
}

#[test]
fn socks5h_environment_uses_proxy_dns_and_reports_proxy_rejection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut socket = accept(listener);
        let mut hello = [0; 2];
        socket.read_exact(&mut hello).unwrap();
        assert_eq!(hello[0], 5);
        let mut methods = vec![0; hello[1] as usize];
        socket.read_exact(&mut methods).unwrap();
        socket.write_all(&[5, 0]).unwrap();
        let mut request = [0; 5];
        socket.read_exact(&mut request).unwrap();
        assert_eq!(
            &request[..4],
            &[5, 1, 0, 3],
            "SOCKS5h must send the hostname to the proxy"
        );
        let mut domain = vec![0; request[4] as usize];
        socket.read_exact(&mut domain).unwrap();
        assert_eq!(domain, b"proxy-target.invalid");
        let mut port = [0; 2];
        socket.read_exact(&mut port).unwrap();
        assert_eq!(u16::from_be_bytes(port), 443);
        socket.write_all(&[5, 2, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
    });
    let output = proxy_child("socks", &format!("socks5h://{address}"));
    server.join().unwrap();
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("account_proxy_failed"));
}

fn tls_server(
    cert: rustls::pki_types::CertificateDer<'static>,
    key: Vec<u8>,
) -> (u16, thread::JoinHandle<()>) {
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert],
        rustls::pki_types::PrivatePkcs8KeyDer::from(key).into(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let thread = thread::spawn(move || {
        let socket = accept(listener);
        let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut stream = rustls::StreamOwned::new(connection, socket);
        let mut request = vec![];
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
            if stream.read_exact(&mut byte).is_err() {
                return;
            }
            request.push(byte[0]);
        }
        if !request.is_empty() {
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK");
            stream.conn.send_close_notify();
            let _ = stream.flush();
        }
    });
    (port, thread)
}

#[tokio::test]
async fn ca_bundle_accepts_its_second_certificate_without_disabling_hostname_checks() {
    let unrelated = rcgen::generate_simple_self_signed(vec!["unrelated.invalid".into()]).unwrap();
    let server = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let bundle = unrelated.cert.pem() + &server.cert.pem();
    let client = with_extra_ca(
        builder(Duration::from_secs(3)).unwrap().no_proxy(),
        Some(bundle.as_bytes()),
    )
    .unwrap()
    .build()
    .unwrap();
    let (port, handle) = tls_server(
        server.cert.der().clone(),
        server.signing_key.serialize_der(),
    );
    let response = client
        .get(format!("https://localhost:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "OK");
    handle.join().unwrap();

    let (port, handle) = tls_server(
        server.cert.der().clone(),
        server.signing_key.serialize_der(),
    );
    let error = client
        .get(format!("https://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap_err();
    assert_eq!(failure("tls.test", &error).kind, FailureKind::Tls);
    handle.join().unwrap();
}

#[tokio::test]
async fn untrusted_certificate_is_a_tls_failure_and_never_falls_back_to_insecure_tls() {
    let server = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let (port, handle) = tls_server(
        server.cert.der().clone(),
        server.signing_key.serialize_der(),
    );
    let error = builder(Duration::from_secs(3))
        .unwrap()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("https://localhost:{port}/"))
        .send()
        .await
        .unwrap_err();
    let diagnostic = failure("tls.test", &error);
    assert_eq!(diagnostic.kind, FailureKind::Tls);
    assert_eq!(diagnostic.detail, "certificate_unknown_issuer");
    handle.join().unwrap();
}

#[test]
fn no_proxy_bypasses_an_explicit_unreachable_proxy() {
    let server = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let (port, handle) = tls_server(
        server.cert.der().clone(),
        server.signing_key.serialize_der(),
    );
    let output = proxy_command("bypass", "http://127.0.0.1:1")
        .env("NO_PROXY", "localhost,127.0.0.1")
        .env("PAB_TEST_HTTP_CA", server.cert.pem())
        .env("PAB_TEST_HTTP_URL", format!("https://localhost:{port}/"))
        .output()
        .unwrap();
    handle.join().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn dns_failure_is_distinguishable_from_a_server_rejection() {
    struct UnavailableDns;
    impl reqwest::dns::Resolve for UnavailableDns {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(async {
                Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "fixture DNS resolution failed",
                )) as Box<dyn Error + Send + Sync>)
            })
        }
    }
    let error = builder(Duration::from_secs(2))
        .unwrap()
        .no_proxy()
        .dns_resolver(Arc::new(UnavailableDns))
        .build()
        .unwrap()
        .get("https://fixture.invalid/")
        .send()
        .await
        .unwrap_err();
    assert_eq!(failure("dns.test", &error).kind, FailureKind::Dns);
}

#[tokio::test]
async fn response_timeout_is_distinguishable_from_tls_and_proxy_failures() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let _socket = accept(listener);
        thread::sleep(Duration::from_millis(300));
    });
    let error = builder(Duration::from_millis(100))
        .unwrap()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{address}/"))
        .send()
        .await
        .unwrap_err();
    assert_eq!(failure("timeout.test", &error).kind, FailureKind::Timeout);
    server.join().unwrap();
}

#[test]
fn configured_ca_must_contain_a_certificate() {
    for bytes in [b"".as_slice(), b"not a certificate".as_slice()] {
        assert_eq!(
            client(Duration::from_secs(1), Some(bytes))
                .unwrap_err()
                .kind,
            FailureKind::InvalidCa
        );
    }
}
