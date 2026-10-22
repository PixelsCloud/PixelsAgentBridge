//! Shared by the Cargo build script and its library tests. Never echo values.
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub control_url: String,
    pub relay_urls: Vec<String>,
    pub relay_ca_cert: Option<PathBuf>,
}

pub fn parse(raw: &str) -> Result<Defaults, &'static str> {
    let defaults: Defaults = toml::from_str(raw.trim_start_matches('\u{feff}'))
        .map_err(|_| "Invalid private client defaults TOML; check syntax, keys and types")?;
    crate::server_urls::validate_pair(&defaults.control_url, &defaults.relay_urls)
        .map_err(|_| "Private client defaults require paired WSS control and HTTPS relay URLs")?;
    if defaults.relay_ca_cert.as_ref().is_some_and(|p| {
        p.as_os_str().is_empty() || p.to_string_lossy().chars().any(char::is_control)
    }) {
        return Err("Private Relay CA requires a nonempty certificate file path");
    }
    Ok(defaults)
}

pub fn read_relay_ca(path: &Path) -> Result<String, &'static str> {
    const ERROR: &str = "Private Relay CA must contain only PEM certificates, never private keys";
    let size = std::fs::metadata(path).map_err(|_| ERROR)?.len();
    if size == 0 || size > 1024 * 1024 {
        return Err(ERROR);
    }
    let pem = std::fs::read_to_string(path).map_err(|_| ERROR)?;
    if pem.contains("PRIVATE KEY") {
        return Err(ERROR);
    }
    let items = rustls_pemfile::read_all(&mut pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ERROR)?;
    if items.is_empty()
        || items
            .iter()
            .any(|item| !matches!(item, rustls_pemfile::Item::X509Certificate(_)))
    {
        return Err(ERROR);
    }
    Ok(pem)
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

    #[test]
    fn private_relay_ca_is_optional_and_paths_are_validated() {
        assert!(parse("").unwrap().relay_ca_cert.is_none());
        assert_eq!(
            parse("relay_ca_cert='relay-ca.pem'").unwrap().relay_ca_cert,
            Some(PathBuf::from("relay-ca.pem"))
        );
        assert!(parse("relay_ca_cert=''").is_err());
        assert!(parse("relay_ca_cert=7").is_err());
    }

    #[test]
    fn private_relay_ca_rejects_keys_invalid_pem_and_oversized_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private-secret-file");
        for raw in [
            "private-secret",
            "-----BEGIN CERTIFICATE-----\n!invalid!\n-----END CERTIFICATE-----",
            "-----BEGIN PRIVATE KEY-----\nprivate-secret\n-----END PRIVATE KEY-----",
        ] {
            std::fs::write(&path, raw).unwrap();
            let error = read_relay_ca(&path).unwrap_err();
            assert!(!error.contains("private-secret"));
        }
        std::fs::write(&path, vec![b'A'; 1024 * 1024 + 1]).unwrap();
        assert!(read_relay_ca(&path).is_err());
    }
}
