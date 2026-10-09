use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
async fn github_http_200_oauth_error_is_classified_without_a_second_exchange() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app = axum::Router::new().route("/token", axum::routing::post(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            Json(json!({"error":"incorrect_client_credentials","error_description":"fixture-sensitive-value"}))
        }
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = GithubConfig::new(
        "fixture-client".into(),
        "fixture-secret".into(),
        "https://pab.example".into(),
        "https://github.com/login/oauth/authorize",
        &endpoint,
        "https://api.github.com/user",
    )
    .unwrap();
    let transport = BoundedTransport(config.http.clone());
    let error = config
        .client
        .exchange_code(AuthorizationCode::new("fixture-code".into()))
        .request_async(&transport)
        .await
        .unwrap_err();
    match error {
        oauth2::RequestTokenError::ServerResponse(error) => {
            assert_eq!(error.error().to_string(), "incorrect_client_credentials")
        }
        _ => panic!("Expected classified provider error, not a malformed success response"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[test]
fn proxy_configuration_rejects_local_dns_socks_and_malformed_urls() {
    for proxy in [
        "socks5://127.0.0.1:1080",
        "ftp://localhost",
        "http://localhost/path",
        "http://localhost/?token=secret",
        "not a url",
    ] {
        assert!(github_http(Some(proxy)).is_err());
    }
    for proxy in [
        "",
        "http://127.0.0.1:1080",
        "https://proxy.example:443",
        "socks5h://127.0.0.1:1080",
    ] {
        assert!(github_http(Some(proxy)).is_ok());
    }
}

#[tokio::test]
async fn socks_proxy_resolves_the_provider_hostname_remotely() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("socks5h://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut greeting = [0; 2];
        socket.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting[0], 5);
        let mut methods = vec![0; greeting[1] as usize];
        socket.read_exact(&mut methods).await.unwrap();
        socket.write_all(&[5, 0]).await.unwrap();
        let mut connect = [0; 5];
        socket.read_exact(&mut connect).await.unwrap();
        assert_eq!(&connect[..4], &[5, 1, 0, 3]); // Domain, not locally resolved IP.
        let mut host = vec![0; connect[4] as usize];
        socket.read_exact(&mut host).await.unwrap();
        assert_eq!(host, b"github-network-test.invalid");
        let mut port = [0; 2];
        socket.read_exact(&mut port).await.unwrap();
        socket
            .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
            .await
            .unwrap();
        let mut request = [0; 4096];
        let n = socket.read(&mut request).await.unwrap();
        assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /profile "));
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
    });
    let client = github_http(Some(&proxy)).unwrap();
    let request = client
        .get("http://github-network-test.invalid/profile")
        .build()
        .unwrap();
    let response = send_github(&client, request, "profile").await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    task.await.unwrap();
}

#[tokio::test]
async fn http_failure_does_not_replay_a_single_use_token_post() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app = axum::Router::new().route(
        "/token",
        axum::routing::post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                StatusCode::SERVICE_UNAVAILABLE
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = github_http(None).unwrap();
    let response = send_github(
        &client,
        client.post(url).body("single-use-code").build().unwrap(),
        "token",
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn dropped_response_does_not_replay_a_token_post() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        drop(socket);
        assert!(
            tokio::time::timeout(Duration::from_millis(600), listener.accept())
                .await
                .is_err()
        );
    });
    let client = github_http(None).unwrap();
    assert!(
        send_github(
            &client,
            client.post(url).body("single-use-code").build().unwrap(),
            "token"
        )
        .await
        .is_err()
    );
    task.await.unwrap();
}

#[tokio::test]
async fn response_timeout_does_not_replay_a_token_post() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        socket.read(&mut request).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(600), listener.accept())
                .await
                .is_err()
        );
        drop(socket);
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    assert!(
        send_github(
            &client,
            client.post(url).body("single-use-code").build().unwrap(),
            "token"
        )
        .await
        .is_err()
    );
    task.await.unwrap();
}

#[tokio::test]
async fn rejected_connection_can_recover_before_sending_the_code() {
    let initial = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = initial.local_addr().unwrap();
    drop(initial);
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let listener = TcpListener::bind(addr).await.unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        socket.read(&mut request).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let response = send_github(
        &client,
        client
            .post(format!("http://{addr}/token"))
            .body("single-use-code")
            .build()
            .unwrap(),
        "token",
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}
