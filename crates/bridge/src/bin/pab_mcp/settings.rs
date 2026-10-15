use pab_agent_core::{DataPaths, DataScope};
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperatorSettings {
    control_url: String,
    relay_url: String,
}

pub fn apply_at_start() -> Result<Vec<String>, Box<dyn std::error::Error>> {
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
        if let Some(settings) =
            pab_service_config::preferences::load::<OperatorSettings>(&path, |s| {
                let config = pab_service_config::client::ClientConfig {
                    control_url: s.control_url.clone(),
                    relay_urls: vec![s.relay_url.clone()],
                    control_ca_cert: None,
                    relay_ca_cert: None,
                    executor: Default::default(),
                    operator: Default::default(),
                };
                config.validate()
            })?
        {
            unsafe {
                env::set_var("PAB_CONTROL_URL", settings.control_url);
                env::set_var("PAB_RELAY_URLS", settings.relay_url);
            }
        }
    }
    Ok(args)
}
