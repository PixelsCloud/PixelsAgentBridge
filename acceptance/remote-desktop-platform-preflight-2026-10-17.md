# Native platform preflight — 2026-10-17

These are read-only capability probes, not remote-desktop acceptance. No
installed application, service configuration or privacy permission was changed.

## Apple

User-session execution on device 419438177 reports macOS 27.0.1 / Apple M4.
Rust 1.95.0 and stable are installed under `/opt/homebrew/opt/rustup/bin`;
the default stable Cargo is 1.99.0. Builds should explicitly select 1.95.0 to
match this checkout. Xcode Command Line Tools provide clang and MacOSX.sdk.

A small Objective-C probe, compiled and run in the existing user session,
reported one active display: ID 1, origin (0,0), logical 1920×1080, pixel
1920×1080, rotation 0. ScreenCaptureKit's SCStream class and VideoToolbox H.264
hardware decoding are available.

`CGPreflightScreenCaptureAccess()` and `AXIsProcessTrusted()` both returned
false for this temporary executable. This does not establish the permissions
of the installed app or its helper. A signed staging app needs its own permission
and LoginWindow checks; no TCC database editing or automatic grant was attempted.

The initial compile failed because an uploaded source file was root-owned and
unreadable by the user. Giving only that source file mode 0644 allowed the user
compile. Both runs removed their exact temporary files and directory afterward.
Successful local record:
`.build/remote-desktop/apple-native-preflight-504ad264627c/result.json`.

Implementation constraint: the Windows viewer embeds a cross-process HWND.
An NSView pointer cannot be passed to another process in the same way. The macOS
viewer needs a native surface owned by the Desktop process (AppKit mutations on
the main thread, codec work off that thread), or an explicit GPU surface-sharing
design. Preserve the existing independent window and toolbar, bounded frame
acknowledgments, and generation/input barriers when adding that host.

## Linux VM

Device 556688188 remains reachable as the existing `hy` user. The earlier
inventory identified Ubuntu 22.04 / X11 and DRM render nodes. This preflight did
not find Cargo in `/home/hy/.cargo/bin`, `/root/.cargo/bin`, or `/usr/bin`, nor
`cc`, NASM or usable X11/Xrandr development pkg-config entries in that user
context. This identifies missing test build prerequisites; it does not establish
that runtime desktop capture or Wayland login is unsupported.

## XRServer

Four outputs were already enumerated and their view-only switching passed in
the multi-screen record. At this check the machine still has LogonUI active
and an existing executor child Python process. The original service is running;
staged tasks are parked. No service restart or input test was attempted against
that busy, locked session.
