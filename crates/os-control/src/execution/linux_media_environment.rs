//! Observe only the graphical environment needed by an unprivileged media
//! helper. Never source a login script or copy service/process secrets wholesale.
use super::{LinuxGraphicalSession, PreparedUser, UserProcess};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxMediaEnvironment {
    session: LinuxGraphicalSession,
    pid: u32,
    marker: String,
    values: BTreeMap<String, String>,
}
impl LinuxMediaEnvironment {
    pub fn observe() -> io::Result<Self> {
        let session = LinuxGraphicalSession::active_console()?;
        let runtime = format!("/run/user/{}", session.uid());
        let info = fs::symlink_metadata(&runtime)?;
        if !info.is_dir() || info.uid() != session.uid() || info.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "graphical runtime directory is unavailable",
            ));
        }
        let mut candidates = Vec::new();
        for entry in fs::read_dir("/proc")? {
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if !entry.metadata().is_ok_and(|m| m.uid() == session.uid()) {
                continue;
            }
            let Ok(comm) = fs::read_to_string(entry.path().join("comm")) else {
                continue;
            };
            let rank = match comm.trim_end() {
                "gnome-shell" => 0,
                "kwin_x11" | "xfce4-session" | "cinnamon" | "mate-session" => 1,
                "gnome-session-b" | "ksmserver" | "plasmashell" => 2,
                _ => continue,
            };
            candidates.push((rank, pid));
        }
        candidates.sort_unstable();
        for (_, pid) in candidates {
            if let Ok(target) = Self::from_process(session.clone(), pid, &runtime) {
                if target.current() {
                    return Ok(target);
                }
            }
        }
        Err(io::Error::other(
            "active graphical environment is not ready",
        ))
    }
    fn from_process(session: LinuxGraphicalSession, pid: u32, runtime: &str) -> io::Result<Self> {
        let marker = crate::process_identity(pid).map_err(io::Error::other)?;
        if LinuxGraphicalSession::for_media_process(pid, session.uid())? != session {
            return Err(io::Error::other(
                "graphical environment belongs to another session",
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(format!("/proc/{pid}/environ"))?
            .take(131073)
            .read_to_end(&mut bytes)?;
        let mut values = select(&bytes, session.kind())?;
        values.insert("XDG_RUNTIME_DIR".into(), runtime.into());
        values.insert("XDG_SESSION_TYPE".into(), session.kind().into());
        values.insert("XDG_SESSION_ID".into(), session.id().into());
        Ok(Self {
            session,
            pid,
            marker,
            values,
        })
    }
    pub fn session(&self) -> &LinuxGraphicalSession {
        &self.session
    }
    pub fn current(&self) -> bool {
        LinuxGraphicalSession::active_console().as_ref().ok() == Some(&self.session)
            && crate::process_identity(self.pid).as_ref().ok() == Some(&self.marker)
            && fs::metadata(format!("/proc/{}", self.pid))
                .is_ok_and(|m| m.uid() == self.session.uid())
            && self.session.current(self.pid)
    }
    pub(super) fn values(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

fn select(bytes: &[u8], kind: &str) -> io::Result<BTreeMap<String, String>> {
    if bytes.len() > 131072 {
        return Err(io::Error::other("graphical environment exceeds bound"));
    }
    let mut values = BTreeMap::new();
    for item in bytes.split(|b| *b == 0) {
        let Some(index) = item.iter().position(|b| *b == b'=') else {
            continue;
        };
        let key = &item[..index];
        if !matches!(
            key,
            b"DISPLAY" | b"XAUTHORITY" | b"DBUS_SESSION_BUS_ADDRESS" | b"WAYLAND_DISPLAY"
        ) {
            continue;
        }
        let value = std::str::from_utf8(&item[index + 1..])
            .map_err(|_| io::Error::other("invalid graphical environment"))?;
        if value.is_empty() {
            continue;
        }
        if value.len() > 4096 || value.chars().any(char::is_control) {
            return Err(io::Error::other("invalid graphical environment value"));
        }
        let key = std::str::from_utf8(key).unwrap();
        if values.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(io::Error::other("ambiguous graphical environment"));
        }
    }
    if let Some(display) = values.get("DISPLAY") {
        let valid = display.strip_prefix(':').is_some_and(|s| {
            let parts: Vec<_> = s.split('.').collect();
            (1..=2).contains(&parts.len())
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        });
        if !valid {
            return Err(io::Error::other("media requires a local X display"));
        }
    }
    if values
        .get("XAUTHORITY")
        .is_some_and(|s| !Path::new(s).is_absolute())
    {
        return Err(io::Error::other("X authority path must be absolute"));
    }
    if values
        .get("DBUS_SESSION_BUS_ADDRESS")
        .is_some_and(|s| s.split(';').any(|v| !v.starts_with("unix:")))
    {
        return Err(io::Error::other("media requires a local session bus"));
    }
    if (kind == "x11" && !values.contains_key("DISPLAY"))
        || (kind == "wayland" && !values.contains_key("DBUS_SESSION_BUS_ADDRESS"))
    {
        return Err(io::Error::other("graphical environment is not ready"));
    }
    Ok(values)
}

impl PreparedUser {
    /// Media-only launch. The ordinary command worker's environment and account
    /// selection remain unchanged. No shell or environment-claimed UID is used.
    pub fn spawn_linux_media(
        &self,
        executable: &Path,
        target: &LinuxMediaEnvironment,
        data: &Path,
        port: u16,
    ) -> io::Result<UserProcess> {
        if !executable.is_absolute()
            || !data.is_absolute()
            || port == 0
            || self.identity().account_id != format!("uid:{}", target.session.uid())
            || !target.current()
        {
            return Err(io::Error::other("media launch context changed"));
        }
        let info = fs::symlink_metadata(data)?;
        if !info.is_dir() || info.uid() != target.session.uid() || info.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "media runtime data is not private to its user",
            ));
        }
        let args = [OsString::from("--media-helper")];
        self.0
            .spawn_linux_media(executable, &args, target, data, port)
            .map(UserProcess)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graphical_environment_is_selected_without_service_secrets_or_identity_claims() {
        let selected = select(b"DISPLAY=:2\0XAUTHORITY=/run/user/128/gdm/Xauthority\0DBUS_SESSION_BUS_ADDRESS=unix:abstract=/tmp/dbus-example\0LD_PRELOAD=/tmp/evil.so\0PAB_DATA_DIR=/root/private\0XDG_SESSION_ID=forged\0PASSWORD=secret\0", "x11").unwrap();
        assert_eq!(selected.len(), 3);
        assert_eq!(selected["DISPLAY"], ":2");
        assert!(!selected.contains_key("PASSWORD"));
        assert!(
            select(
                b"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus\0",
                "wayland"
            )
            .is_ok()
        );
        assert!(select(b"DISPLAY=:0.1\0", "x11").is_ok());
        for bytes in [
            b"DISPLAY=remote:0\0".as_slice(),
            b"DISPLAY=:1\0DISPLAY=:2\0",
            b"DISPLAY=:1\0XAUTHORITY=relative\0",
            b"DISPLAY=:1\0DBUS_SESSION_BUS_ADDRESS=tcp:host=remote\0",
            b"DISPLAY=:1\0DBUS_SESSION_BUS_ADDRESS=unix:path=/x;tcp:host=remote\0",
            b"DISPLAY=:1\n\0",
            b"DISPLAY=:1..0\0",
        ] {
            assert!(select(bytes, "x11").is_err());
        }
        assert!(select(b"DISPLAY=:1\0", "wayland").is_err());
        assert!(select(&vec![0; 131073], "x11").is_err());
    }
}
