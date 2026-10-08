//! Native credential sharing acceptance harness. Uses only a fake test token.
//! Sign copies as Desktop and MCP; never point this at real account storage.
use pab_agent_core::account::{AccountSession, AccountStore, AccountUser};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err(
            "usage: account-store-probe write|read|delete <isolated-directory> <test-https-origin>"
                .into(),
        );
    }
    let store = AccountStore::new(std::path::Path::new(&args[1]), &args[2])?;
    match args[0].as_str() {
        "write" => {
            store.login(AccountSession {
                user: AccountUser {
                    id: pab_protocol::UserId::new(),
                    username: "credential-probe".into(),
                    server_admin: false,
                },
                access_token: "a".repeat(64),
            })?;
        }
        "read" => {
            let state = store.read()?;
            let token = store
                .token(state.active_slot.as_deref().ok_or("missing slot")?)?
                .ok_or("missing token")?;
            assert_eq!(token.as_str(), "a".repeat(64));
        }
        "delete" => {
            let state = store.logout()?;
            for slot in state.pending_logouts {
                store.finish_logout(&slot)?;
            }
        }
        _ => return Err("unknown action".into()),
    }
    println!("PASS: {}", args[0]);
    Ok(())
}
