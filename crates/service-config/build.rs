#[path = "build_support.rs"]
mod build_support;
#[path = "src/server_urls.rs"]
mod server_urls;

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=PAB_CLIENT_DEFAULTS_FILE");
    println!("cargo:rerun-if-changed=build_support.rs");
    println!("cargo:rerun-if-changed=src/server_urls.rs");
    let explicit = env::var_os("PAB_CLIENT_DEFAULTS_FILE");
    let path = explicit.clone().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("../../.env/client-defaults.toml")
    });
    assert!(
        !path.as_os_str().is_empty(),
        "PAB_CLIENT_DEFAULTS_FILE must name a TOML file"
    );
    // Track an absent optional file too, so creating/removing it rebuilds every
    // dependent executable. Do not emit URLs via cargo:rustc-env or build logs.
    println!("cargo:rerun-if-changed={}", path.display());
    let defaults = match fs::read_to_string(&path) {
        Ok(raw) => {
            assert!(
                raw.len() <= 64 * 1024,
                "Private client defaults exceed 64 KiB"
            );
            build_support::parse(&raw).unwrap_or_else(|error| panic!("{error}"))
        }
        Err(e) if explicit.is_none() && e.kind() == std::io::ErrorKind::NotFound => {
            build_support::Defaults::default()
        }
        Err(_) => panic!("Cannot read private client defaults TOML"),
    };
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("client_defaults.rs");
    // Debug string literals escape TOML values as data, never as Rust source.
    fs::write(output, format!(
        "pub const DEFAULT_CONTROL_URL: &str = {:?};\npub const DEFAULT_RELAY_URLS: &[&str] = &{:?};\npub const DEFAULT_RELAY_URL: &str = {:?};\n",
        defaults.control_url, defaults.relay_urls, defaults.relay_urls.join(",")
    )).expect("Cannot generate client defaults");
}
