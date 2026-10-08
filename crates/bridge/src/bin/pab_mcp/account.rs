use pab_agent_core::account::{AccountClient, AccountStore};
use std::io::{IsTerminal, Read};
use zeroize::Zeroizing;

pub(super) async fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let store = AccountStore::from_env()?;
    match args.first().map(String::as_str) {
        Some("status") if args.len() == 1 => {
            let state = store.read()?;
            println!("{}", serde_json::json!({"revision":state.revision,"user":state.user}));
        }
        Some("logout") if args.len() == 1 => {
            let state = store.logout()?;
            pab_agent_core::account::notify_account_change(state.revision);
            let pending = match AccountClient::from_env() {
                Ok(client) => client.flush_logouts(&store).await.is_err(),
                Err(_) => true,
            };
            println!("{}", serde_json::json!({"signed_out":true,"server_revocation_pending":pending}));
        }
        Some(action @ ("login" | "register")) if args.len() == 3 && args[2] == "--password-stdin" => {
            if std::io::stdin().is_terminal() { return Err("pipe the password on stdin; passwords are never accepted in command-line arguments".into()); }
            let mut password = Zeroizing::new(String::new());
            std::io::stdin().take(1027).read_to_string(&mut password)?;
            while password.ends_with(['\r','\n']) { password.pop(); }
            if password.is_empty() || password.len() > 1024 { return Err("invalid password length".into()); }
            let client = AccountClient::from_env()?;
            let session = client.login(&args[1], password, action == "register").await?;
            let state = client.save_session(&store, session).await?;
            pab_agent_core::account::notify_account_change(state.revision);
            let _ = client.flush_logouts(&store).await;
            println!("{}", serde_json::json!({"revision":state.revision,"user":state.user}));
        }
        _ => return Err("usage: pab-mcp account status|logout; pab-mcp account login|register <username> --password-stdin".into()),
    }
    Ok(())
}
