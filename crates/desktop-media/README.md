# Native desktop media

This crate provides native capture, codecs, cursor and input adapters for remote
desktop sessions. The reliable transport/session protocol lives in
[`pab-remote-desktop`](../remote-desktop/README.md); this crate owns platform media.

## Process boundary

`pab-media-worker` is the standalone worker entry point. Desktop embeds the same
native library and dispatches `--media-worker` before initializing Tauri. Workers
exchange bounded messages and compressed frames over inherited private pipes;
desktop pixels do not travel through WebView IPC, logs or task history.

`NativeBackend` runs in the active session helper. Executor validates the existing
authenticated local IPC boundary before exposing media to a device session.
Capture, cursor and input operations share session/generation checks. Dropping
a source releases its worker; callers must preserve this lifetime boundary.

## Platforms

- Windows: DXGI capture, NVENC hardware encoding, Media Foundation decoding and
  native D3D presentation, with bundled OpenH264 fallback where supported.
- macOS: native capture, VideoToolbox and Metal adapters, with platform permission
  and interactive-session requirements.
- Linux: separate X11, GNOME/PipeWire and DRM adapters. Availability depends on
  the session, compositor, driver and permissions; capabilities must be queried.

The presence of an adapter does not guarantee support on every driver, OS release,
display layout or login screen. Preserve explicit unsupported/error results.
Synthetic capture, decoder-failure and replay modes are developer diagnostics.

## Build and tests

Use the project [build instructions](../../BUILDING.md). Product builds use
the native sources in this crate and pinned dependencies; local experiment
reports are not required build inputs. Platform tests and synthetic fixtures
are under `tests/`; hardware, permissions and login transitions require their
actual target environment in addition to these deterministic checks.

Keep third-party licenses and version information from
[`native/vendor/README.md`](native/vendor/README.md) with redistributed components.
