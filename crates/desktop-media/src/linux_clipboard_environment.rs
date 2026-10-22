//! Supplement only this clipboard child's display addresses from its own user
//! manager. Never evaluate shell output or import unrelated environment data.
use std::{collections::BTreeMap, path::Path};

fn select(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    if bytes.len() > 131_072 { return result; }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else { return result; };
    if value["type"] != "as" { return result; }
    let Some(values) = value["data"].as_array() else { return result; };
    for entry in values.iter().filter_map(|v| v.as_str()) {
        let Some((key, value)) = entry.split_once('=') else { continue; };
        if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) { continue; }
        let valid = match key {
            "DISPLAY" => value.strip_prefix(':').is_some_and(|v| {
                let parts: Vec<_> = v.split('.').collect();
                (1..=2).contains(&parts.len()) && parts.iter().all(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            }),
            "XAUTHORITY" => Path::new(value).is_absolute(),
            "WAYLAND_DISPLAY" => value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
            _ => false,
        };
        if valid { result.insert(key.to_owned(), value.to_owned()); }
    }
    result
}

pub(super) fn initialize() {
    // Existing graphical app environments are already authoritative. A session
    // bus is required; never fall back to the system/root user's manager.
    if (std::env::var_os("DISPLAY").is_some() && std::env::var_os("WAYLAND_DISPLAY").is_some())
        || std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() { return; }
    let Ok(output) = std::process::Command::new("/usr/bin/busctl").args([
        "--user", "--json=short", "get-property", "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "Environment",
    ]).output() else { return; };
    if !output.status.success() { return; }
    for (key, value) in select(&output.stdout) {
        if std::env::var_os(&key).is_none() {
            // This function runs before arboard starts threads, in the dedicated
            // worker process, never in the multi-threaded Desktop/service.
            unsafe { std::env::set_var(key, value); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_addresses_only_without_shell_evaluation_or_identity_changes() {
        let bytes = serde_json::to_vec(&serde_json::json!({"type":"as","data":[
            "DISPLAY=:0.1", "WAYLAND_DISPLAY=wayland-0", "XAUTHORITY=/run/user/1000/.mutter-Xwaylandauth.test",
            "LD_PRELOAD=/tmp/injected", "PAB_DATA_DIR=/root/private", "DBUS_SESSION_BUS_ADDRESS=unix:path=/other",
            "PASSWORD=secret", "DISPLAY=$(touch /tmp/injected)", "WAYLAND_DISPLAY=../other-user/socket"
        ]})).unwrap();
        let selected = select(&bytes);
        assert_eq!(selected.len(), 3);
        assert_eq!(selected["DISPLAY"], ":0.1");
        assert_eq!(selected["WAYLAND_DISPLAY"], "wayland-0");
        assert!(select(&vec![b' '; 131_073]).is_empty());
        assert!(select(b"not-json").is_empty());
    }
}
