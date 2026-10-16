# Windows software codec acceptance — 2026-10-17

This checkpoint adds independently selectable software encoding and decoding
to the production Windows media worker. Normal codec initialization tries the
existing hardware path first. Forced combinations below exercise the actual
Rust adapter, private pipe, codec and native swapchain, not a stand-alone codec
throughput estimate. They do not establish automatic runtime device-loss
recovery, product GUI software-path input, or macOS/Linux support.

## Implementation and build

- OpenH264 0.9.8 / openh264-sys2 0.9.9, bundled source, no runtime download.
  Source revisions and licenses are recorded in the media vendor README and
  distributed third-party notice. Unix packages now include the same notice.
- Successful NVENC encoding still directly registers the BGRA capture texture.
  Software encoding alone reads it back, converts limited BT.709 I420 and emits
  Annex B. Software decoding produces NV12 and uploads a texture for the native
  video processor. Software paths do not claim zero copies.
- Four fixed encoding slices enable actual parallelism: the upstream wrapper
  kept a single slice despite requesting four threads. Priming output stays
  private; the first published frame is a fresh IDR. Small frames use one slice.
- x86/x64 requires NASM before building, including the dependency build. NASM
  3.01 was used from an isolated local directory. Compilation used two Cargo
  jobs; other builds were checked against the shared maximum of ten.
- Eight media unit tests pass, including BT.709 primaries, padded/truncated
  rows, bounds, changing-frame round trips, multi-slice first-frame independent
  decoding, and immutable decoder-path reporting.
- Python package checks: archive packaging 1 passed; Linux archive 1 passed;
  macOS package checks 5 passed / 4 host-specific checks skipped on Windows.
  The unified build/version regression suite also passed all 15 tests.
  These are not actual macOS/Linux installation acceptance.

An early software-decoder render failed with `0x80070057`: shader-resource-only
textures cannot be used as video-processor input. Default-usage NV12 textures
with zero bind flags fixed it, following the
[Microsoft input-view contract](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11videodevice-createvideoprocessorinputview).

## 90: real 1920 × 1080 output

The test window ran in the logged-in Administrator session. The installed
service and application files were not replaced. Exact temporary executables
were hash-checked after upload, then removed after the run; no test processes
remained. Each 15 FPS run submits 90 frames, excludes the initial 15-frame
warm-up from FPS, and observes 90 physical display confirmations.

| Capture / decoder | Source | Effective displayed FPS | Max sampled RGB error |
| --- | --- | ---: | ---: |
| NVENC / D3D11 | synthetic changing colors | 14.918 | 2 |
| NVENC / OpenH264 | synthetic changing colors | 14.923 | 2 |
| OpenH264 / D3D11 | synthetic changing colors | 14.748 | 4 |
| OpenH264 / OpenH264 | synthetic changing colors | 14.731 | 4 |
| OpenH264 / OpenH264 | animated desktop duplication | 14.932 | not sampled |

The synthetic runs check four decoded/displayed colors each frame (360 checks).
The animation run performs zero pixel readbacks for verification. All five
pass the unchanged criteria: >12 effective FPS and at least 90% of requested
FPS. Result: `.build/remote-desktop/native-stream-90-b03571831dac/report.json`.

Validated artifact SHA-256:

- `pab-media-worker.exe`: `80e196f7f82bb0f4a379110ecaad750b8ac85c4f4b72b53f8558ea8ad3569653`
- `native_stream_check.exe`: `2ffe3f67f1cc013cee88ac309977d49e606c47870f32e1f828a7972542defddc`

A separate 30 FPS synthetic all-software run displayed all 180 frames at
22.966 FPS. It exceeds 12 FPS but **fails the 30 FPS acceptance threshold**;
it is not reported as a 30 FPS pass. Its temporary files were also cleaned.
The separate 30 FPS animated-desktop run did pass at 29.734 FPS, with all 180
frames displayed and zero verification pixel readbacks; its temporary files
were cleaned. These different workloads are kept separate rather than claiming
that every 1080p software workload sustains 30 FPS.

## Limits observed rather than hidden

The full Windows Desktop Release build also completed. A snapshot of the actual
embedded-worker executable (not installed locally) has SHA-256
`f466c4e07de382f9c0beb12818d4e5a20b61ebcbe50426443129c6fb2770ffb5`,
27,092,480 bytes. Using the new diagnostic `--embedded` option on local 4K:
hardware-encode/software-decode passes at 14.679 FPS (max RGB error 3); animated
all-software capture/decode passes at 13.514 FPS, with all 90 frames displayed.
No Cargo/rustc/native compiler process was active during that latter run.
This validates the shipped binary's media entry point, not GUI keyboard input.
The updated diagnostic executable SHA-256 is
`069bd9f84a0a24fd5cfb3183981bd35595efe317311ea55bb17fefcd5ceb3e01`.

- Before enabling multiple slices, 90 software encoding reached only 13.106
  FPS and failed the 15 FPS criterion. This motivated the slice fix.
- Local 3840 × 2160 hardware-encode/software-decode passed at 14.636 FPS with
  max sampled RGB error 3. The earlier standalone full-software 4K dynamic run
  reached 11.332 FPS while a two-job Desktop build was active locally. No
  resolution reduction or threshold relaxation was used. The idle-machine
  embedded-binary result above is the subsequent dynamic-workload result.
  The embedded 4K full-frame color-change stress test still fails at 6.398 FPS,
  despite displaying all 90 frames with max RGB error 4. Do not generalize the
  dynamic-desktop pass to every 4K software workload.
- Initialization failure fallback is implemented, but forced-path tests do not
  prove every unsupported GPU selects it correctly. Hardware runtime failure,
  no-D3D11 machines, asynchronous MFT events, weak-network recovery and product
  GUI software-input acceptance remain open under W3/W7.
- XRServer remains locked, with an existing executor child Python process.
  Its original service is running and staged tests are parked. Four-screen
  viewing from the earlier checkpoint stands; four-screen click acceptance
  remains pending an available unlocked session.

No release or COS upload was performed for this checkpoint.
