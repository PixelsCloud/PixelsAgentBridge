# Runtime codec recovery — 2026-10-17

Baseline: `f8d88a3` (bundled software codecs and initialization fallback).
This record does not mark W3 or the full remote-desktop task complete.

## Encoder API failure

Capture now retires a failed NVENC session and encodes the same captured texture
with a fresh OpenH264 encoder. Failed partial output is replaced, and the first
software packet contains SPS/PPS and an IDR. Frame numbering, display geometry
and the authenticated transport/session stay unchanged. No GPU readback occurs
on a successful hardware frame. If capture/device access itself has failed,
ordinary media-error recovery still applies; this is not a no-GPU display path.

The explicit `encoder-failure` diagnostic requires successful NVENC startup,
encodes two hardware frames, then throws at the third encode attempt. It asserts
that frame 3 is a keyframe and verifies all subsequent decoded colors/order.
Ordinary product capture never enables this injected failure.

On 90, 1920×1080 / target 15 FPS:

| Source | Displayed frames | Effective displayed FPS | Sampled RGB error |
| --- | ---: | ---: | ---: |
| Full-frame color changes | 90 | 14.716 | max 4 / 360 checks |
| Animated desktop duplication | 90 | 14.899 | no verification readback |

Both runs report NVENC startup, software fallback, and exactly 88 software
frames. All frames display in one native decoding stream. Temporary files and
processes were cleaned; installed binaries and service settings were unchanged.
Local record: `.build/remote-desktop/native-stream-90-e1578091cb3b/report.json`.

Artifact SHA-256:

- Worker: `1fdcd4d22441bd16867535f8afb2bb9046e7796c190412b659ba7011cca0ce15`
- Diagnostic: `cb2e1c28c6902c57f72988861a312a719da210e85764a7618dd47abd61064961`

## Viewer failure

Implementation now distinguishes native renderer/decoder errors from transport,
remote-source and authentication errors. A native failure selects software once
for that window, closes the old remote session, and opens a fresh view-only
session. A subsequent software-renderer failure is surfaced, avoiding an endless
hardware/software restart loop. User retry remains available. Native input
events use the same private event pipe in software mode.

The full Windows debug Desktop build completed with eight Cargo jobs (nested
CMake constrained to one; no other compiler was active). Its executable SHA-256
is `3e34f36f2398ed8ac99b50c0ec14f054a80bf2bb8cf67ab559f9896f8c7780f6`.
This build validates recovery behavior; release FPS remains the separate record.

In a fresh isolated Desktop profile, viewing 90 through the staged older source:

1. The initial hardware renderer reached first-frame-ready and enabled input.
   A click reached only the bounded test window at physical coordinate (960,540).
2. F24 was deliberately held on that window. Terminating only this profile's
   renderer changed the window to reconnecting, closed the old remote session,
   and delivered F24-up to the remote fixture. The new session reached ready with
   input disabled. Its owned worker command is `stream-interactive-software`.
3. Enabling input again and clicking the same native surface increased the
   fixture's click count at (960,540), proving the software-renderer event pipe.
4. F24 was held again and only the software renderer was terminated. The remote
   fixture received key-up; its held-release ledger was zero. An 18-second
   bounded watch observed failure with input disabled and no automatic restart.
   No renderer worker remained. Existing command/status requests still worked.
5. Manual retry opened another fresh software session, ready and view-only.
   The viewer was then closed and the isolated local Desktop process stopped.

Observed session suffixes: hardware `...59082038`, software recovery
`...e890096f`, manual retry `...2fc3c843`. Private records are under
`.build/remote-desktop/viewer-decoder-fallback-20261017`; the bounded failure
watch is `software-crash-watch.json`. Remote fixture counters ended with three
F24-down messages (one duplicate press) and two F24-up transitions, with no held
key remaining. Input events were never replayed into the recovered session.
Afterward 90's original service command and running state were verified restored,
all five staged tasks and the exact temporary directory were removed, and no
owned local/remote test process remained. The failure watch has 59 samples;
all 55 failed-state samples have input disabled and not ready.

No GPU-driver crash, hardware hot-unplug, or no-D3D11 machine has been tested.
Those cases and weak-network/resource/soak acceptance remain open.
