use super::*;
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn legacy_and_modern_clients_report_identity_before_any_tool_call() {
    for (name, method) in [
        ("kimi-code", "initialize"),
        ("codex_cli_rs", "initialize"),
        ("claude-code", "server/discover"),
        ("dsh-mcp-client", "tools/list"),
        ("cursor-agent", "initialize"),
        ("opencode", "initialize"),
    ] {
        tokio::time::timeout(Duration::from_secs(10), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (reporter, reporting) = McpReporter::start_at(format!("ws://{}/ws/mcp", listener.local_addr().unwrap()));
            let server = McpServer {
                tool_settings: Arc::new(Default::default()), runtime: Default::default(),
                reporter, runtime_reporting: Default::default(), operations: Default::default(),
            };
            let (client_io, server_io) = tokio::io::duplex(256 * 1024);
            let serving = tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap(); });
            let (read, mut write) = tokio::io::split(client_io);
            let mut lines = BufReader::new(read).lines();
            let params = if method == "initialize" {
                json!({"protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":name,"version":"client-test"}})
            } else {
                json!({"_meta":{
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientInfo":{"name":name,"version":"client-test"},
                    "io.modelcontextprotocol/clientCapabilities":{}
                }})
            };
            write.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})).as_bytes()).await.unwrap();
            let response: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert!(response.get("result").is_some(), "{response}");
            if method == "initialize" {
                write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
            }
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            loop {
                let msg = socket.next().await.unwrap().unwrap();
                if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                    let report: Value = serde_json::from_str(&text).unwrap();
                    if report["clientName"] == name {
                        assert_eq!(report["clientVersion"], "client-test");
                        assert!(report["runtime"].is_null());
                        break;
                    }
                }
            }
            drop(write); drop(lines);
            serving.await.unwrap();
            reporting.shutdown().await;
        }).await.expect(name);
    }
}
