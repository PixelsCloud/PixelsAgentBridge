use pab_agent_core::{DataPaths, DataScope};
use pab_service_config::operator::OperatorSettings;
use std::env;

pub fn apply_at_start() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    for (key, value) in pab_service_config::operator::launch_defaults(|key| env::var_os(key)) {
        unsafe {
            env::set_var(key, value);
        }
    }
    let explicit = env::args_os().any(|a| a == "--config");
    let (args, values) = pab_service_config::client::startup(
        pab_service_config::client::Role::Operator,
        env::args_os().skip(1),
    )?;
    // Called before Tokio starts threads.
    for (key, value) in values {
        unsafe {
            env::set_var(key, value);
        }
    }
    if !explicit {
        let path = DataPaths::for_scope(DataScope::User)?
            .root()
            .join("operator-server.toml");
        for (key, value) in OperatorSettings::load(&path)?.environment() {
            unsafe {
                env::set_var(key, value);
            }
        }
    }
    Ok(args)
}
