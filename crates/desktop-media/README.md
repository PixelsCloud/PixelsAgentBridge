# Native desktop media

The Windows worker implements DXGI texture capture, direct BGRA-texture NVENC
H.264 input, Media Foundation D3D11 decoding and native swapchain display.
Codec initialization failures fall back independently to bundled OpenH264.
Software encoding reads back BGRA and converts it to limited BT.709 I420;
software decoding uploads NV12 for the same native presenter. These CPU copies
are absent from the successful direct hardware path. Runtime device-loss
fallback and systems without usable D3D11 capture/display remain unfinished.
An NVENC API failure now retires that encoder and begins software output with
a new IDR at the same frame number. The Desktop viewer handles native renderer
failure by closing the old session and trying software once in a fresh view-only
session; another software failure is surfaced until user retry. Transport/auth
errors retain their existing recovery policy. See the codec-recovery acceptance
record for the injected encode failure and real renderer-process crash tests.
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

On Windows, `NativeBackend::cursor` requires the exact active video configuration.
Its cursor-only DXGI worker observes shape/position/visibility independently of
video requests. It keeps only the latest snapshot, releases each desktop lease
before pipe output, and never copies/maps/encodes the desktop texture. This uses
an additional duplication client and GPU device; resource/performance impact
still needs product acceptance under concurrent capture workloads. A cursor
failure does not restart video or modify its direct NVENC path.

DXGI pointer top-left is converted to a desktop-space hotspot. Separate drawing
is hidden whenever DXGI says the hardware pointer is not visible, including
when it is already embedded in video. Color shapes are converted to premultiplied
BGRA; monochrome and masked-color shapes retain AND/XOR semantics. Shape and
image buffers are bounded, with no position history. The video lease invalidates
and reaps retained cursor sources even while idle. `cursor-shape-check` uses
generated shapes without touching the OS pointer; the Windows cursor integration
test includes an explicitly ignored real-GPU lifecycle check. Windows native
viewer overlays are connected, as are the Mac Metal and Linux OpenGL overlays.
Linux GNOME Wayland and X11 independent capture are connected; X11 system-hide
visibility, DRM cursor planes and full product acceptance remain pending.

The Mac backend now shares this cursor process/lease adapter and advertises it
through the existing session helper. Its video worker selects
`desktop-separate-cursor` (ScreenCaptureKit `showsCursor = NO`); direct diagnostic
`desktop` capture retains its embedded cursor. The cursor worker observes
`NSCursor.currentSystemCursor`, global Core Graphics position and visibility,
without accessing video surfaces. It rasterizes the bounded NSImage at the
selected display's capture scale, preserving logical size, hotspot and top-down
premultiplied BGRA; a high-resolution TIFF representation is not its screen size.
Only changed shapes cross the pipe. Display geometry changes retire the source.

`CGCursorIsVisible` is a deprecated Apple API with no replacement; it is resolved
at runtime and a missing symbol fails the optional cursor source. RustDesk's
`CGSCurrentCursorSeed` optimization is also resolved at runtime, with pixel
comparison when unavailable. A changing seed disables cache reuse without
discarding an otherwise complete image snapshot. These compatibility paths and
older macOS still need real-platform acceptance. Generated shape checks and the
Mac build pass; live Mac source/video coexistence, hidden/shown/motion and retained
lease teardown remain pending while the Apple test machine is locked. See
`acceptance/remote-desktop-macos-cursor-source-2026-10-18.md` from the repo root.

On GNOME Wayland, the capture worker extracts cursor metadata from the existing
PipeWire stream (`cursor-mode=2`). Cursor-only buffers are returned without
replacing pending video pixels. One immutable shape and latest position are read
through a private per-capture Unix socket, restricted to the capture parent's
PID/UID. There is no second capture, full-frame cursor readback or video-credit
dependency. The Rust reader shares video cancellation, generation validation and
bounded framing. Protected-desktop DRM fallback
reports independent cursor unavailability and keeps its existing video path.
The real Ubuntu worker lease/reopen/motion checks passed; this does not establish
formal viewer, X11, DRM cursor-plane or long-run acceptance. See
`acceptance/remote-desktop-linux-cursor-source-2026-10-18.md`.

The DRM exporter now has a native cursor-plane extraction component. It enables
the virtual-driver hotspot capability only on its own observer fd, selects the
currently bound cursor plane, and exports its bounded image/render descriptors.
Unobservable capability is distinct from a known-but-unbound (hidden) cursor.
The unprivileged reader can CPU-map linear cursor DMA-BUFs with START/END READ
sync; this handles vmwgfx dumb cursor buffers which cannot be imported into EGL.
Other supported cursor images retain the EGL import path; video behavior is
unchanged. The component is not yet wired into the broker's single-exporter
scheduling or the formal cursor channel. Native VM evidence and remaining work:
`acceptance/remote-desktop-linux-drm-cursor-native-2026-10-18.md`.

On X11, a separate cursor worker shares the existing process/lease adapter.
It uses its own X connection/process, keeping the process-global Xlib error
handler away from the video/input connections. XFixes shape events update a
bounded premultiplied BGRA image; pointer-only updates reuse that image. Video
omits cursor blending in `desktop-separate-cursor` mode, and cursor movement
does not generate encoded frames on a static desktop. XWayland is never used
to represent a Wayland desktop. Transparent shapes and departure from the
selected display/root hide the overlay; layout changes retire the cursor.

XFixesHideCursor is an outstanding visibility limitation: the server continues
to return the old image without notifying a shape change. It must not be
reported as full hidden-cursor support. Disposable two-screen Xvfb checks cover
the complete Desktop's worker pipe, lease/reopen/motion, cancellation, and video
pixel comparison, not physical multi-monitor or formal remote-window acceptance.
See `acceptance/remote-desktop-linux-x11-cursor-source-2026-10-18.md` and
`experiments/remote-desktop/check_x11_cursor_source.py`.

`NativeViewer::cursor_renderer()` returns a generation-owned independent
submitter. Windows Desktop reads the remote cursor stream alongside video and
input; source errors clear the last image while preserving the session. A local
partial pipe failure instead retires that renderer generation. Cursor commands
share only the serialized native pipe write, not the pending video read/decoder
acknowledgment. Private event 6 confirms cursor consumption and cannot grant
video credit or increment displayed-video FPS.

The GPU overlay blends premultiplied colors directly onto the output; AND/XOR
shapes copy only their clipped background rectangle to a GPU texture. No output
pixels are read by the CPU. Position/visibility changes reuse the shape, retain
one decoded frame for static repaint, and clip to the visible screen rather
than its letterbox. Viewport scaling, desktop/capture coordinate ratios and
hotspots are applied without rotating the cursor shape again. The local pointer
is hidden only over a controlled viewport with an active remote cursor stream;
view-only mode and cursor-source failure restore its normal pointer. The shader
uses the Windows D3DCompiler library and adds no downloaded runtime component.

`NativeViewer::open_in_window` renders into a native child of the Desktop-owned
window. The top 72 logical pixels belong to its controls. The surface preserves
aspect ratio, resizes on DPI/window changes, retains one decoded sample for
static repaint, and counts only confirmed presentations as displayed frames.
Minimized frames are decoded/consumed but are not counted as displayed FPS.
Presentation statistics retain at most three pending IDs; only specifically
observed IDs count. Missing observations evict old IDs without counting them or
blocking rendering. The swapchain latency handle bounds actual GPU submissions.
A render acknowledgment confirms consumption/submission, not physical display.
`finish()` polls statistics for at most two seconds, then discards unconfirmed IDs.

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
Its optional final argument selects `software-encode`, `software-decode` or
`software` to test each mixed pair explicitly. This is a diagnostic argument,
not persisted application configuration. A final `--embedded` tests a Desktop
executable through its `--media-worker` entry point. `software_codec_check` measures only
CPU conversion/codec time; its result is not capture or displayed FPS.
`encoder-failure` starts on real NVENC, injects failure at the third frame, and
checks that fallback starts with a keyframe and continues the decoded sequence.

Building the bundled software codec on x86/x64 requires NASM on PATH before
Cargo starts. Rebuild `openh264-sys2` after installing NASM if an older cache was
built without it; upstream otherwise silently disables x86 assembly. All Cargo
and nested compilation must stay within the shared 10-job limit. OpenH264 is
compiled from the pinned dependency source, with no runtime codec download.
Its four encoding slices use at most four worker threads. The first published
frame is forced to IDR after configuring slices; no priming frame is transmitted.

Current limitations, still tracked in `REMOTE_DESKTOP_LONG_TERM_TASK.md`:
NVIDIA hardware encoding, system D3D11 MFT decoding, SDR BT.709,
Mac/Linux cursor overlays and remaining platform acceptance. Windows native keyboard,
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
translation unit using the Windows SDK and links the bundled CPU codec plus
system libraries.

The macOS worker now has display inventory, ScreenCaptureKit NV12/IOSurface
capture and direct VideoToolbox H.264 encoding. The Aqua/LoginWindow product
helper is connected, while real desktop permission acceptance,
system login and older-system/software encoder fallback are still unfinished.
The application keeps its macOS 12.0 minimum; the current hardware media worker
requires macOS 13 to configure the tested single-reference low-latency stream.
ScreenCaptureKit is weak-linked, verified in the Cargo-built Mach-O executable.
No TCC grant is fabricated and no missing permission is reported as a frame.

`cargo run --release -p pab-desktop-media --example native_capture_check -- WORKER 15`
tests native synthetic encoding through the production private pipe and bundled
OpenH264 decoding. It checks every frame's four quadrants and reports codec
throughput, not desktop capture/display FPS. The Apple-generated fixture in
`tests/fixtures` also checks immediate ordered software output on Windows/macOS.
See `acceptance/remote-desktop-macos-codec-2026-10-17.md` for measured Windows
native display interoperability and the still-unmet macOS product requirements.

Mac native decoding/display now uses hardware VideoToolbox NV12 IOSurfaces and
Metal texture views, with bundled OpenH264 on initial hardware failure. The
worker supports `render WIDTH HEIGHT FPS stream|stream-verify [0] [ROTATION]`;
`-software`, `-fail-decoder` and `-lifecycle` are explicit developer diagnostics.
No NSView pointer is accepted over the worker pipe. The Mac product viewer now
creates its view inside the owning Tauri main thread, with bounded background
decode and local mouse/keyboard callbacks. Default view-only and existing input
leases are preserved. The integrated Desktop builds and launches on Apple;
real remote product-window/input and system-login acceptance remain unfinished.
See `acceptance/remote-desktop-macos-host-2026-10-17.md` for the host integration,
local event checks, lifecycle recheck and remaining limits.
See `acceptance/remote-desktop-macos-renderer-2026-10-17.md` and
`experiments/remote-desktop/verify_macos_renderer.py` for generated-frame replay,
hardware/software, rotation, malformed-packet and static-window checks.

The Linux native worker now implements GNOME DisplayConfig inventory and
ScreenCast v4 / PipeWire capture through the same private compressed pipe.
This is the CPU fallback: mapped BGRx is converted by the bundled BT.709 software
encoder. It keeps one pending pool buffer, releases it before pipe writes, and
retires the source when the layout, session or format changes. Linux Desktop now
registers its media helper using the existing process identity challenge plus
kernel UID and logind graphical-user observations. Idle registrations retire on
session changes too. GNOME native input now uses a separate worker with
compositor-owned virtual devices, selected-stream coordinates and release on
layout changes, EOF or worker death. Linux rendering and system login remain open.
Build dependencies are gio-2.0, libpipewire-0.3 and NASM; the existing Linux
Desktop image and package already include PipeWire. GNOME's interface is private
and versioned; this adapter does not claim support for every Wayland desktop.
See `acceptance/remote-desktop-linux-capture-2026-10-17.md` for 1080p VM results,
real cancellation/layout checks and remaining work. The native boundary checks
in `tests/native_linux.cpp` can run with ASan/UBSan without a desktop session.
See `acceptance/remote-desktop-linux-helper-2026-10-17.md` for real helper IPC,
isolated Desktop helper startup and embedded-worker tests. These do not yet
constitute an end-to-end remote window or system-login acceptance.
Real GTK input and crash/layout release checks are recorded in
`acceptance/remote-desktop-linux-input-2026-10-17.md`. This GNOME adapter does not
yet provide X11/general Wayland or login-manager input acceptance.

Linux also contains a split DRM DMA-BUF path (`drm_export.h`, `dma_frame.h`,
`dma_image.h`, `dma_capture.h`). A root exporter passes read-only image fds plus
the matching render-node fd over an inherited packet socket; a graphical-user
worker reuses its EGL context and emits the existing compressed-pipe format.
The Linux supervisor owns a local packet socket in each root-owned helper
profile. Its broker checks the peer UID, process group and active graphical
session, pins the compositor with a pidfd, and launches the root `export-session`
child. A compositor control fd never reaches the unprivileged capture worker.
The normal GNOME capture path tries this fallback if PipeWire startup fails,
before publishing any compressed header. Internal `export-dma` / `capture-dma`
entries remain available for isolated component checks. No new application
config or remote authentication/transport is introduced.

The DMA path checks power, lock state and layout before publishing a frame. It
rejects unsupported primary-plane crops/scaling/rotation until those transforms
are implemented. Native descriptor tests (`tests/native_linux_dma.cpp`) exercise
real SCM_RIGHTS and fd cleanup under ASan/UBSan. The VM passed user-process
encoding/decoding at about 15 FPS at 1280×800 and old-generation rejection after
locking, but login, 1080p network presentation and complete supervisor/product
acceptance remain open. See `acceptance/remote-desktop-linux-dma-native-2026-10-17.md`
for the original native checks. Broker lifecycle and helper-routing follow-up:
`acceptance/remote-desktop-linux-dma-broker-2026-10-17.md`.
