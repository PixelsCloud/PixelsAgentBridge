//! Per-capture Unix channel. Only runtime paths live here, no application config.
use super::*;
use std::os::unix::fs::DirBuilderExt;

pub(super) struct Channel {
    root: PathBuf,
    pub(super) path: PathBuf,
}
impl Channel {
    pub(super) fn new() -> std::io::Result<Self> {
        let root = PathBuf::from("/tmp").join(format!("pab-cursor-{}", RequestId::new()));
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        let path = root.join("cursor.sock");
        Ok(Self { root, path })
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        // Unique create-only directory; never recurse or follow its contents.
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.root);
    }
}

pub(super) fn supported() -> bool {
    // XWayland cannot represent the whole Wayland desktop. X11 keeps embedded
    // capture until its independent-source visibility behavior is validated.
    std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("x11")
        && (std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()))
}
