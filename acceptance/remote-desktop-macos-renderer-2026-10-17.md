# macOS native decoding and Metal display — 2026-10-17

Baseline: `69faeea`. W4 remains incomplete. This adds the native display module;
the Mac product window, input, permission and system-login integration are still
pending. No installed application, service, authentication or transport changed.

## Implementation

- VideoToolbox requires a hardware H.264 decoder. Decoded limited-range NV12
  IOSurfaces become Metal R8/RG8 texture views. A shader converts BT.709 to RGB,
  scales with letterboxing and rotates in four directions. The hardware path
  does not map/copy decoded pixels through CPU memory. Compressed bytes are
  copied into the decoder sample; driver-internal copies are not measured.
- The first hardware initialization/decode failure retires its partial state
  and replays that same packet through bundled OpenH264. Software NV12 output
  is copied into an IOSurface pool capped at four live buffers. Decoder choice
  stays fixed for the generation; runtime hardware failure is an explicit error
  for the future product recovery path to handle.
- Packet bounds, Annex-B framing, SPS/PPS geometry, color and IDR transitions
  are checked before selecting a decoder, including explicitly forced software.
  Neither a malformed stream nor a changed resolution is accepted by fallback.
- GPU completion retains pixel buffers and their Metal texture views. UI work
  stays on the AppKit main thread. Embedded NSViews will belong to that process;
  passing a cross-process parent pointer is rejected.
- Decoder consumption, render completion and actual display counts are separate.
  Only a positive `CAMetalDrawable.presentedTime` counts as displayed. Repeated
  redraws of the same frame do not increase this count. Drawable submission IDs
  additionally order asynchronous results across resize/static redraws.
- Initial attachment, resize, backing-scale, occlusion and restore request a
  redraw of the retained last surface. Dropped drawables can request a redraw
  at a bounded rate. Hidden/minimized windows skip GPU submission; static waits
  pump the UI without requesting another remote frame.

Pixel verification is opt-in and only used with generated quadrants. It reads
four pixels from the rendered drawable; the ordinary hardware display path
has no CPU pixel readback. Raw desktop images are not saved by these checks.

## Machine and checks

Apple M4, macOS 27.0.1, one 1920×1080 screen at scale 1.0. Rust 1.95.0 Release,
two build jobs; Windows checks also used two jobs, never exceeding ten combined.
Windows and Mac media unit tests each passed 9/9 after the shared codec ABI header extraction
and new private decoder-path value. The Windows native media behavior itself
was not changed.

Initial native 1080p tests reached only 10.5–10.8 FPS. The worker unnecessarily
pumped/slept for each already-buffered field. Waiting only when its pipe has no
data fixed that bottleneck. Early passing runs still displayed 89/90 or 179/180
frames: the later single-frame test exposed an unpresented first drawable. This
failure was not counted as a display. Layer attachment/visibility, dropped-frame
redraw and static restore handling were subsequently added and retested.

The single-frame lifecycle suite uses hardware/software decode in all four
rotations. It changes the window between portrait/landscape sizes, verifies
GPU colors, confirms resized drawable presentation, minimizes/restores the
same static frame and closes the window. Repeated redraws must still report
one distinct frame, 16 sampled color checks, and no GPU draw while minimized.
These checks do not simulate a real rotated/mixed-DPI monitor or system login.

Final static/rotation worker:
`a6030261046dd985c63e93fd656c0ab1b88fd0dd42f71d074c7686109cd96213`.
All eight lifecycle cases passed with exactly one displayed frame, 16 color
checks and maximum RGB error 1. Hardware continuous display results:

| Source | Rotation | Target FPS | Displayed | Effective display FPS | Color checks / max error |
| --- | ---: | ---: | ---: | ---: | --- |
| Apple 1920×1080 | 0 | 15 | 90/90 | 14.892 | 360 / 2 |
| Apple 1920×1080 | 90 | 15 | 90/90 | 14.712 | 360 / 2 |
| Apple 1920×1080 | 180 | 15 | 90/90 | 14.864 | 360 / 2 |
| Apple 1920×1080 | 270 | 15 | 90/90 | 14.734 | 360 / 2 |
| Windows 3840×2160 | 0 | 30 | 180/180 | 30.010 | 720 / 6 |
| Windows 3840×2160, no pixel readback | 0 | 30 | 180/180 | 29.871 | disabled |

30.010 is a finite-window timing result, not a claim to exceed the 30 FPS source.

Software display on the same final worker:

| Source | Target FPS | Displayed | Effective display FPS | Color checks / max error |
| --- | ---: | ---: | ---: | --- |
| Apple 1920×1080 | 15 | 90/90 | 14.923 | 360 / 2 |
| Apple 1920×1080 | 30 | 180/180 | 29.528 | 720 / 2 |
| Windows 3840×2160 | 15 | 89/90 | 14.644 | 360 / 3 |
| Windows 3840×2160 | 30 | 180/180 | 29.240 | 720 / 6 |

The 4K/15 software run lost one physical presentation while continuing to the
next frame; it is retained as 89/90, not silently promoted to 90. The static
single-frame cases pass independently, including recovery of a dropped drawable.

The malformed-input suite covers oversized/truncated packets, wrong sequence,
missing Annex-B prefix, empty/forbidden NAL, no initial IDR, wrong geometry and
duplicate frame index under both decoder selections (18 cases). Each invalid
frame must fail without decoder acceptance or presentation credit.

Initialization fault injection happens after a real hardware decode succeeds,
then verifies software replay of the original first frame. All four 1080p/4K,
15/30 FPS cases passed on worker
`d9a8d9e0099f6ba29d00856ee866ee7479658190188c0dee6986aa6db67c988c`,
with 14.73/29.33 FPS (1080p) and 14.69/29.65 FPS (4K), maximum RGB errors 2/6.
Those figures predate the static-frame fix and retain the initial dropped frame.

On the final `a6030261…` worker, the four initialization-failure cases passed
again at 14.680/29.955 FPS (1080p) and 15.126/29.638 FPS (4K), with all 90/180
frames displayed. All 18 malformed-input cases passed again. The complete
40-case final report is [saved with artifact hashes](remote-desktop-macos-renderer-2026-10-17.json).

The existing Rust `native_stream_check` also passed using that worker and
example SHA-256 `ae95e183c136fe36e1c18d87c83637dd46b4009062ec243fdccd929763d03734`.
This exercises live generated hardware encoding, the production private pipe
and bounded Rust renderer adapter, hardware decoding and native Metal display:
1080p/15 achieved 14.791 FPS, all 90 frames/360 color checks; 1080p/30 achieved
27.203 FPS, all 180 frames/720 checks. Maximum RGB error was 2. It is still local
generated media, not network-connected product desktop acceptance.

## Reproduction and boundaries

`experiments/remote-desktop/verify_macos_renderer.py` is standard-library Python.
Its `capture` action **always** asks for generated synthetic quadrants, never a
real desktop. It refuses to replace fixtures. Generate on Apple and Windows:

```text
python verify_macos_renderer.py capture /path/to/pab-media-worker /tmp/mac-fixtures
python verify_macos_renderer.py capture C:/path/to/pab-media-worker.exe C:/temp/win-fixtures --prefix windows
```

Transfer the four generated `.pabvideo` files into one Mac directory, then run
`render /path/to/pab-media-worker /tmp/combined-fixtures --suite SUITE` for
`rotations`, `software`, `failure`, `invalid` and `lifecycle`. Each child has a
bounded lifetime; only that child/window is terminated. The reports are runtime
test data, with binary/fixture hashes, counts and timing, not application config.

The throughput checks pace six-second streams and use a one-second warmup.
Their conservative rate subtracts all warmup frames from confirmed display
counts, then divides by the remaining wall time. Required rate is both >12 FPS
and at least 90% of target. Results describe generated local display/codec
interoperability, not live capture/network latency, full-screen natural video
complexity, product acceptance, GPU device loss, or an extended soak test.

Raw local task records: `.build/remote-desktop/apple-worker-a5edf16be3dc/`.
The isolated Apple source/build cache is retained for product integration.
No TCC database changes or fabricated Screen Recording grants were made.

Remaining W4 work: in-process product view embedding and bounded native input,
real desktop capture with ordinary application permissions, capture software
fallback/older macOS support, helper registration, screen/session transitions,
LoginWindow/lock/login and both real product connection directions. Linux, AI
assistance, the full stability matrix and release remain tracked separately.

Primary API references: [CAMetalLayer](https://developer.apple.com/documentation/quartzcore/cametallayer),
[actual presented time](https://developer.apple.com/documentation/metal/mtldrawable/presentedtime),
[Core Animation presentation coordination](https://developer.apple.com/documentation/quartzcore/cametallayer/presentswithtransaction).
