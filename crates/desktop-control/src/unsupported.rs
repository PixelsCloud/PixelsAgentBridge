use pab_protocol::WindowControlAction;
pub const BACKEND: &str = "xcap/control_unsupported";
pub fn mark(_: u32, _: u32, _: &str, _: u32) -> Result<(), String> {
    Err("stable external window control is not supported on this platform".into())
}
pub fn verify(id: u32, pid: u32, key: &str, marker: u32) -> Result<(), String> {
    mark(id, pid, key, marker)
}
pub fn unmark(_: u32, _: u32, _: &str, _: u32) {}
pub fn focused(_: u32) -> Result<bool, String> {
    Err("foreground verification unsupported".into())
}
pub fn pointer_targets_window(_: u32) -> Result<bool, String> {
    Err("pointer target verification unsupported".into())
}
pub fn act(
    id: u32,
    pid: u32,
    key: &str,
    marker: u32,
    _: Option<WindowControlAction>,
) -> Result<(), String> {
    mark(id, pid, key, marker)
}
