# Native desktop media

The Windows worker implements DXGI texture capture, direct BGRA-texture NVENC
H.264 input, Media Foundation D3D11 decoding and native swapchain display.
`pab-media-worker` is the standalone developer binary. Desktop embeds the same
native library and dispatches `--media-worker` before initializing Tauri; the
installer therefore needs no extra executable or downloaded SDK.

The worker has inherited private stdin/stdout pipes. Frames are compressed,
bounded to 16 MiB, and pulled individually by the parent. No pixels pass through
WebView IPC, logs, task output or files. Inventory/control data is runtime
protocol data, not application configuration. All application settings remain
in the project's existing TOML configuration.

`NativeBackend` runs inside an active session helper, never in Session 0.
Executor's dedicated `local_ipc::media` connection checks the existing HMAC
access and kernel PID/account challenge before exposing this backend to an
already-authenticated device session. Its ordinary window/screenshot queue is
unaffected. Dropping a source kills its worker and the backend gate waits for
process exit before another worker starts in that helper.

`NativeViewer::open_in_window` renders into a native child of the Desktop-owned
window. The top 72 logical pixels belong to its controls. The surface preserves
aspect ratio, resizes on DPI/window changes, retains one decoded sample for
static repaint, and counts only confirmed presentations as displayed frames.
Minimized frames are decoded/consumed but are not counted as displayed FPS.
Presentation statistics are observed asynchronously, with at most three pending
IDs; only specifically observed IDs count. A render acknowledgment confirms
consumption/submission, not physical display. `finish()` drains statistics.

Current limitations, still tracked in `REMOTE_DESKTOP_LONG_TERM_TASK.md`:
NVIDIA capture only, synchronous low-latency decoder output, SDR BT.709,
no cursor channel or keyboard/mouse yet, no native macOS/Linux backend.
GPU rotation is pixel-tested in all four orientations; actual rotated monitor
capture and multi-monitor hotplug still require hardware acceptance.
Windows secure-desktop routing is implemented but needs lock/login acceptance.
The synthetic/animate modes are explicit developer diagnostics; the product
`NativeBackend` always starts ordinary desktop capture.

Vendor source/license details are in `native/vendor/README.md`. The native
sources here are independent of `experiments/` and `.build/`; Cargo compiles one
translation unit using the Windows SDK and links only system libraries.
