//! Read-only native probe used by Desktop and isolated acceptance fixtures.
use pab_agent_core::server_probe::{probe, ServerKind};

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let kind = match args.first().map(String::as_str) {
        Some("control") => ServerKind::Control,
        Some("relay") => ServerKind::Relay,
        _ => { eprintln!("usage: server-probe control|relay URL [CA.pem]"); std::process::exit(2); }
    };
    let Some(url) = args.get(1) else { std::process::exit(2); };
    let ca = args.get(2).map(std::fs::read).transpose().expect("read test CA");
    match probe(kind, url, ca.as_deref()).await {
        Ok(result) => println!("{}", serde_json::to_string(&result).unwrap()),
        Err(error) => { println!("{}", serde_json::to_string(&error).unwrap()); std::process::exit(1); }
    }
}
