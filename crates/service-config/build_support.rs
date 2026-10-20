//! Shared by the Cargo build script and its library tests. Never echo values.
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub control_url: String,
    pub relay_urls: Vec<String>,
}

pub fn parse(raw: &str) -> Result<Defaults, &'static str> {
    let defaults: Defaults = toml::from_str(raw.trim_start_matches('\u{feff}'))
        .map_err(|_| "Invalid private client defaults TOML; check syntax, keys and types")?;
    crate::server_urls::validate_pair(&defaults.control_url, &defaults.relay_urls)
        .map_err(|_| "Private client defaults require paired WSS control and HTTPS relay URLs")?;
    Ok(defaults)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_or_empty_defaults_stay_unconfigured() {
        for raw in [
            "",
            "control_url = ''\nrelay_urls = []",
            "\u{feff}# public build",
        ] {
            let d = parse(raw).unwrap();
            assert!(d.control_url.is_empty() && d.relay_urls.is_empty());
        }
    }

    #[test]
    fn private_defaults_are_paired_validated_and_errors_redacted() {
        let d =
            parse("control_url='wss://private.test/control'\nrelay_urls=['https://relay.test']")
                .unwrap();
        assert_eq!(d.control_url, "wss://private.test/control");
        for raw in [
            "control_url='wss://secret.test'",
            "relay_urls=['https://secret.test']",
            "control_url='https://secret.test'\nrelay_urls=['https://relay.test']",
            "control_url='wss://user:secret@host.test'\nrelay_urls=['https://relay.test']",
            "unknown_secret='private-secret'",
            "control_url='private-secret",
        ] {
            let error = parse(raw).err().unwrap();
            assert!(!error.contains("secret") && !error.contains("host.test"));
        }
    }
}
