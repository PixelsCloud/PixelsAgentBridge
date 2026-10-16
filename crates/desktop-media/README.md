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

The production render pipe uses version 2 events: decoder acceptance and
decoded/rendered consumption are separate. `NativeViewer::submit()` waits for
acceptance; `presentations()` independently wakes the session's generation-bound
acknowledgment handle, even while a video read is pending. At most four packet
headers wait for output. Acceptance alone cannot grant network credit or mark
the first frame ready. A screen change or close invalidates old acknowledgment
handles. The Windows system MFT may return NEED_MORE_INPUT or NOTACCEPTING;
the worker drains available output and bounds stalls instead of demanding one
immediate output for each input. This is not an event-driven asynchronous MFT
implementation; hardware MFT event negotiation remains future work.

`cargo run -p pab-desktop-media --example native_stream_check -- WORKER synthetic 15`
explicitly tests the production adapter on the primary screen. `animate` uses
real desktop duplication of a temporary animation; neither mode saves video.
The report uses actual screen dimensions and DXGI-confirmed display counts.

Current limitations, still tracked in `REMOTE_DESKTOP_LONG_TERM_TASK.md`:
NVIDIA capture only, system D3D11 MFT decoding, SDR BT.709,
no cursor channel yet, no native macOS/Linux backend. Windows native keyboard,
mouse and held-input recovery are implemented and tested on 90.
GPU rotation is pixel-tested in all four orientations; actual rotated monitor
capture and multi-monitor hotplug still require hardware acceptance.
Windows secure-desktop routing and continuous recovery in the product viewer
passed lock/login acceptance on 90, including the streaming output adapter;
see the 2026-10-17 lock/login and decoder-output acceptance records.
The synthetic/animate modes are explicit developer diagnostics; the product
`NativeBackend` always starts ordinary desktop capture.

Vendor source/license details are in `native/vendor/README.md`. The native
sources here are independent of `experiments/` and `.build/`; Cargo compiles one
translation unit using the Windows SDK and links only system libraries.
