use pab_agent_core::account::{AccountClient, AccountStore};
use std::io::{IsTerminal, Read};
use zeroize::Zeroizing;

pub(super) async fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let store = AccountStore::from_env()?;
    match args.first().map(String::as_str) {
        Some("devices") if args.len()>=2 => {
            let paths=pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)?;
            let local=pab_bridge::BridgeLocalStore::open(&paths.bridge_database()).await?;
            match args[1].as_str() {
                "status" if args.len()==2=>println!("{}",serde_json::to_string(&local.catalog_status().await?)?),
                "sync" if args.len()==2=>{local.sync_account_catalog().await?;println!("{}",serde_json::to_string(&local.catalog_status().await?)?);},
                "import-local" if args.len()==2=>{let count=local.import_local_devices().await?;let pending=local.sync_account_catalog().await.is_err();println!("{}",serde_json::json!({"queued":count,"sync_pending":pending}));},
                action @ ("keep-local"|"keep-cloud") if args.len()==3=>{local.resolve_catalog_conflict(args[2].parse()?,action=="keep-local").await?;println!("{}",serde_json::json!({"resolved":true}));},
                _=>return Err("usage: pab-mcp account devices status|sync|import-local; pab-mcp account devices keep-local|keep-cloud <device-id>".into()),
            }
        }
        Some("status") if args.len() == 1 => {
            let state = store.read()?;
            println!("{}", serde_json::json!({"revision":state.revision,"user":state.user}));
        }
        Some("associate") if args.len() <= 2 => {
            let action=match args.get(1).map(String::as_str) {
                None=>pab_protocol::DeviceAccountAction::Associate,
                Some("--replace")=>pab_protocol::DeviceAccountAction::Replace,
                _=>return Err("usage: pab-mcp account associate [--replace]".into()),
            };
            let current=pab_agent_core::account::local_device::associate(pab_protocol::DeviceAccountAction::Automatic,None).await?;
            let Some(current)=current else {return Err("sign in before associating this device".into());};
            let result=pab_agent_core::account::local_device::associate(action,Some(current.revision)).await?;
            println!("{}",serde_json::to_string(&result)?);
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
            let association=pab_agent_core::account::local_device::associate(pab_protocol::DeviceAccountAction::Automatic,None).await;
            println!("{}", serde_json::json!({"revision":state.revision,"user":state.user,"device_association":association.as_ref().ok(),"association_pending":association.is_err()}));
        }
        _ => return Err("usage: pab-mcp account status|logout; pab-mcp account login|register <username> --password-stdin; pab-mcp account associate [--replace]".into()),
    }
    Ok(())
}
