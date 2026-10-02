use pab_protocol::WindowControlAction;
use x11rb::{
    connection::Connection, protocol::xproto::*, rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};
pub const BACKEND: &str = "xcap/x11rb/enigo";
fn connect() -> Result<(RustConnection, u32), String> {
    let (c, s) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = c.setup().roots[s].root;
    Ok((c, root))
}
fn atom(c: &RustConnection, name: &str) -> Result<u32, String> {
    Ok(c.intern_atom(false, name.as_bytes())
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom)
}
fn values(c: &RustConnection, id: u32, name: &str, kind: AtomEnum) -> Result<Vec<u32>, String> {
    let a = atom(c, name)?;
    let r = c
        .get_property(false, id, a, kind, 0, 256)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    Ok(r.value32().map(|v| v.collect()).unwrap_or_default())
}
fn valid(c: &RustConnection, id: u32, pid: u32, name: &str, marker: u32) -> Result<(), String> {
    if values(c, id, "_NET_WM_PID", AtomEnum::CARDINAL)?.first() != Some(&pid)
        || values(c, id, name, AtomEnum::CARDINAL)?.as_slice() != [marker]
    {
        return Err("window reference invalidated (closed or replaced)".into());
    }
    Ok(())
}
pub fn mark(id: u32, pid: u32, name: &str, marker: u32) -> Result<(), String> {
    let (c, _) = connect()?;
    if values(&c, id, "_NET_WM_PID", AtomEnum::CARDINAL)?.first() != Some(&pid) {
        return Err("window identity changed".into());
    }
    let a = atom(&c, name)?;
    c.change_property32(PropMode::REPLACE, id, a, AtomEnum::CARDINAL, &[marker])
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    valid(&c, id, pid, name, marker)
}
pub fn verify(id: u32, pid: u32, name: &str, marker: u32) -> Result<(), String> {
    let (c, _) = connect()?;
    valid(&c, id, pid, name, marker)
}
pub fn unmark(id: u32, pid: u32, name: &str, marker: u32) {
    if let Ok((c, _)) = connect() {
        if valid(&c, id, pid, name, marker).is_ok() {
            if let Ok(a) = atom(&c, name) {
                if let Ok(cookie) = c.delete_property(id, a) {
                    let _ = cookie.check();
                }
            }
        }
    }
}
pub fn focused(id: u32) -> Result<bool, String> {
    let (c, root) = connect()?;
    Ok(values(&c, root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW)?.first() == Some(&id))
}
pub fn pointer_targets_window(id: u32) -> Result<bool, String> {
    let (c, mut current) = connect()?;
    for _ in 0..32 {
        if current == id {
            return Ok(true);
        }
        let hit = c
            .query_pointer(current)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        if !hit.same_screen || hit.child == 0 {
            return Ok(false);
        }
        current = hit.child;
    }
    Ok(false)
}
fn send(c: &RustConnection, root: u32, id: u32, name: &str, data: [u32; 5]) -> Result<(), String> {
    let event = ClientMessageEvent::new(32, id, atom(c, name)?, data);
    c.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )
    .map_err(|e| e.to_string())?
    .check()
    .map_err(|e| e.to_string())?;
    c.flush().map_err(|e| e.to_string())
}
fn exists(c: &RustConnection, id: u32) -> Result<bool, String> {
    match c
        .get_window_attributes(id)
        .map_err(|e| e.to_string())?
        .reply()
    {
        Ok(_) => Ok(true),
        Err(x11rb::errors::ReplyError::X11Error(e))
            if e.error_kind == x11rb::protocol::ErrorKind::Window =>
        {
            Ok(false)
        }
        Err(e) => Err(e.to_string()),
    }
}
pub fn act(
    id: u32,
    pid: u32,
    name: &str,
    marker: u32,
    action: Option<WindowControlAction>,
) -> Result<(), String> {
    let (c, root) = connect()?;
    valid(&c, id, pid, name, marker)?;
    let supported = values(&c, root, "_NET_SUPPORTED", AtomEnum::ATOM)?;
    let required = match action {
        None => "_NET_ACTIVE_WINDOW",
        Some(WindowControlAction::Close) => "_NET_CLOSE_WINDOW",
        _ => "_NET_WM_STATE",
    };
    if !supported.contains(&atom(&c, required)?) {
        return Err("window manager does not support requested EWMH operation".into());
    }
    let horz = atom(&c, "_NET_WM_STATE_MAXIMIZED_HORZ")?;
    let vert = atom(&c, "_NET_WM_STATE_MAXIMIZED_VERT")?;
    let hidden = atom(&c, "_NET_WM_STATE_HIDDEN")?;
    match action {
        None => send(&c, root, id, "_NET_ACTIVE_WINDOW", [1, 0, 0, 0, 0])?,
        Some(WindowControlAction::Close) => {
            send(&c, root, id, "_NET_CLOSE_WINDOW", [0, 1, 0, 0, 0])?
        }
        Some(WindowControlAction::Minimize) => {
            send(&c, root, id, "WM_CHANGE_STATE", [3, 0, 0, 0, 0])?
        }
        Some(WindowControlAction::Maximize) => {
            send(&c, root, id, "_NET_WM_STATE", [1, horz, vert, 1, 0])?
        }
        Some(WindowControlAction::Restore) => {
            c.map_window(id)
                .map_err(|e| e.to_string())?
                .check()
                .map_err(|e| e.to_string())?;
            send(&c, root, id, "_NET_WM_STATE", [0, horz, vert, 1, 0])?;
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if action == Some(WindowControlAction::Close) && !exists(&c, id)? {
            return Ok(());
        }
        valid(&c, id, pid, name, marker)?;
        let states = values(&c, id, "_NET_WM_STATE", AtomEnum::ATOM)?;
        let confirmed = match action {
            None => values(&c, root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW)?.first() == Some(&id),
            Some(WindowControlAction::Close) => false,
            Some(WindowControlAction::Minimize) => states.contains(&hidden),
            Some(WindowControlAction::Maximize) => states.contains(&horz) && states.contains(&vert),
            Some(WindowControlAction::Restore) => {
                !states.contains(&hidden)
                    && !states.contains(&horz)
                    && !states.contains(&vert)
                    && c.get_window_attributes(id)
                        .map_err(|e| e.to_string())?
                        .reply()
                        .map_err(|e| e.to_string())?
                        .map_state
                        == MapState::VIEWABLE
            }
        };
        if confirmed {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(
                "window manager did not confirm requested state; action may still be pending"
                    .into(),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
