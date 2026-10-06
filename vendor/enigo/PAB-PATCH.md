# Pixels Agent Bridge patch to Enigo 0.5.0

Source: https://crates.io/crates/enigo/0.5.0 (MIT; see LICENSE).
The registry crate checksum is recorded in the previous Cargo.lock revision.
Source, upstream tests and license are retained; registry metadata and its own
lockfile are omitted. Do not edit the Cargo registry cache at build time.

Local change: Settings.macos_use_session_event_tap selects the public Quartz
Session event tap for every Enigo event, including releases. Default false keeps
upstream HID behavior on normal desktops. Only the active LoginWindow agent opts
in, together with independent_of_keyboard_state=false. No new event construction,
key mapping, permissions bypass, or private APIs are introduced.

Why: LoginWindow rejected the private event source; combined source construction
succeeded but HID posting did not affect its login fields. Session posting is
also used by RustDesk's macOS input service:
https://github.com/rustdesk/rustdesk/blob/master/src/server/input_service.rs
Public API: https://developer.apple.com/documentation/coregraphics/cgeventtaplocation

Session posting alone did not restore login input in our acceptance test. The
app also needed the pre-login Mach-O declaration in its build.rs; version 1.2.13
with both changes passed actual logout/login and post-login text/save checks.
Do not infer that this patch alone fixes LoginWindow or bypasses TCC.

Both workspace manifests patch Enigo because Tauri is a separate Cargo workspace.
Remove this patch when upstream supports selecting the event tap.

Windows absolute mouse input now normalizes against SM_X/Y/CX/CYVIRTUALSCREEN
and sends MOUSEEVENTF_VIRTUALDESK. The upstream 0.5.0 primary-only normalization
cannot address monitors at negative origins. Invalid/empty geometry is rejected
before SendInput; single-pixel extents do not divide by zero. Relative input and
key handling are unchanged. Unit tests cover negative origins and both edges;
PAB's owned-window acceptance observes the actual click.
Public contract: https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput
Compare upstream implementation: https://github.com/enigo-rs/enigo/blob/main/src/win/win_impl.rs
