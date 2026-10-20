# macOS native codec foundation — 2026-10-17

Baseline: `fd2156a`. W4 remains incomplete. Installed applications/services,
authentication, transport and Windows production media logic were not changed.

## Implemented

- Stable CG display IDs, logical bounds, physical pixel dimensions and scale.
  ScreenCaptureKit images are already oriented, so their protocol rotation is
  identity; actual portrait/mixed-DPI displays are not yet tested on Apple.
- ScreenCaptureKit supplies limited-range BT.709 NV12 IOSurfaces. A one-frame
  mailbox retains the latest surface and passes its CVPixelBuffer directly to
  VideoToolbox. The hardware desktop path does not map/read back CPU pixels.
  Generated diagnostic frames deliberately map CPU memory to fill their colors.
- Bounded Annex-B packets use the existing pull pipe and sequence/keyframe
  checks. Capture callbacks own their state across asynchronous completion;
  static-screen waits pump the run loop and sleep to avoid busy polling.
- Hardware-required VideoToolbox low-latency mode, High profile, no reordering,
  one temporal base layer, LTR disabled and one reference buffer. The last
  property requires macOS 13; older-system media fallback remains unfinished.
- The application minimum remains macOS 12.0. Apple availability annotations
  and the native unit's 12.0 deployment target produce `LC_LOAD_WEAK_DYLIB` for
  ScreenCaptureKit; checked with `otool -l` on the Cargo-built executable.

No new project configuration format was introduced. Inventory, pipe framing,
test reports and the generated compressed fixture are runtime/test data.

## Evidence

Apple: M4, macOS 27.0.1, display ID 1, 1920×1080, scale 1.0, name 29C1.
Rust 1.95.0 Release builds used two jobs, including the native unit. Windows
checks also used two jobs; aggregate compilation stayed below ten jobs.
Both platforms passed all nine media unit tests, including eight generated
VideoToolbox frames checked against four expected colors per frame.

Standalone Apple hardware encoding produced 90/180 frames at 15.024/29.964 FPS
for targets 15/30. Downloaded **generated** streams were then paced into the
existing Windows native render protocol; this separates codec interoperability
from network and real desktop capture acceptance.

| Target FPS | Windows decoder | Physically displayed | Effective display FPS | RGB checks / maximum error |
| ---: | --- | ---: | ---: | --- |
| 15 | D3D11 hardware | 90 | 14.854 | 360 / 3 |
| 15 | OpenH264 software | 90 | 14.857 | 360 / 3 |
| 30 | D3D11 hardware | 180 | 29.774 | 720 / 3 |
| 30 | OpenH264 software | 180 | 29.695 | 720 / 3 |

The Cargo worker with the final codec/run-loop settings encoded 90/180 generated frames at
15.021/29.803 FPS. These are codec rates, not proof of actual desktop/display FPS.
Its actual desktop startup returned an explicit Screen Recording permission
error and zero pipe bytes. No permission dialog, TCC grant or installed-binary
replacement was performed to make this pass.

The committed `native_capture_check` example was built on both platforms and
run on Apple against the final Cargo worker. Sequential native encoding plus
OpenH264 software decoding/color checks passed target 15 at 15.017 FPS. Target
30 reached 19.355 FPS with all 180 frames and 720 color checks correct (maximum
YUV error 0), but **failed** its 90%-of-target threshold. This CPU diagnostic
still exceeds 12 FPS; it is not evidence of 30 FPS Mac native display. Native
hardware decoding/display remains a separate implementation and acceptance.
After bounding per-frame Objective-C autoreleases, the finalized worker passed
the 15 FPS encode/decode check again at 15.006 FPS with zero sampled YUV error.

Artifacts:

- Standalone Apple encoder: `cbe37ce18d7adb3a6660586d5bbb941c59488d9cadd7d1aa0dd8793acd53f2bc`.
- Cargo Apple codec/run-loop worker: `9f96888256339780868e0a9e3677109ba6c3d59f8c97381eb5570a92a916921c`.
- Final Cargo Apple worker (per-frame autorelease pools): `5fc3a1dde368869d36cdcc575f6cc354881588b1eb7399db99e13fe3e6192a91`.
- Windows decoder worker: `251d0240d9e23349aa706f3804ad10d80b74e1ce9d49bfc406968398a9dad575`.
- Committed eight-frame generated fixture: `5d6a912240fea08acbe57c00a4e8920d303d252252ef7fe9baa5d84c30681c25`.
- Local records: `.build/remote-desktop/apple-worker-a5edf16be3dc/`.

All 90/180 Annex-B packets from the Cargo codec/run-loop worker matched the
standalone interoperability run byte for byte (capture timestamps excluded).
Three earlier exploratory remote directories were removed. The final isolated
source/build cache remains for subsequent native viewer work, with no worker
left running and no installed app/service replacement.

## Failures retained and remaining acceptance

Normal VideoToolbox Main/Baseline streams stalled Windows hardware decoding
inside the unchanged four-frame budget. Changing Windows low-latency settings
or adding AUD did not solve it. Apple's low-latency mode fixed hardware output,
but its default reference buffering caused stale OpenH264 images; disabling
OpenH264's automatic flush introduced delayed output and did not establish a
working low-latency path. Those exploratory Windows changes were removed.
Explicit one-reference encoding passed both existing decoder paths.

The native helper is not yet advertised by the product on macOS. Still needed:
real desktop capture under the correct app's ordinary Screen Recording grant,
native view/decoder integration, input and held-key release, multi-monitor,
screen/session transitions, LoginWindow/lock/login, software/older-system
fallback and both product connection directions. FileVault pre-boot cannot be
inferred from a logged-in capture or from this codec benchmark.

Platform references: [Apple ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit),
[VideoToolbox low-latency selection](https://developer.apple.com/documentation/videotoolbox/kvtvideoencoderspecification_enablelowlatencyratecontrol),
[framework weak linking](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPFrameworks/Concepts/WeakLinking.html).
