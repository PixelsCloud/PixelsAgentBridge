# Finder AX window selection fix

## Reproduction

Installed Mac ARM 1.2.25 rejected the Finder Applications window with
`interactive_window_unavailable` through native `pab_ui_query`, operation
`aa3bec77-b33a-46f5-893f-9c3d9fdec76f`. The screen was awake and the window had a
valid helper reference. A separately signed, read-only diagnostic under the
existing fixed certificate inspected AXWindows: three AXWindow entries and one
AXScrollArea; one AXWindow matched the requested title. AX access was trusted.
The AXScrollArea had no title (AX error -25212). No user document content was read.

The old loop returned immediately for any non-AXWindow entry, discarding a valid
candidate already found. This was our selector bug, not missing permission or an
unsupported Finder window.

## Change

Skip non-window AX objects when selecting the requested root. Require exactly
one AXWindow matching the verified process, title and geometry. An all-non-window
list remains unavailable; duplicate matching windows remain ambiguous; provider
errors remain errors rather than proof of uniqueness. Existing reference,
process-identity and containment checks are unchanged.

Three Mac regression tests passed: unrelated non-window entries before/after the
target; all-non-window/no-match/duplicate targets; failure after a partial match.
Test task: `79890c90-22c2-47a6-ad0f-8141232929e9`.

## Installed verification

Built ARM and Intel complete debug packages as **1.2.26**, using the existing
fixed signing certificate. The single build reservation advances build_count to
27; local manifests mirror that reservation, without allocating another version.
ARM installed successfully (exit 0), with device identity and designated signing
requirement preserved. Installed executable hashes match the build manifest.
No TCC reset or renewed authorization was needed. Intel was built/signed only.

Native `pixels.pab_ui_query` now succeeds on the same Finder window:
`0c7a3160-50d2-4dcb-83ee-9a5e155c6a57`, 50 elements returned, correctly marked
`truncated=true`, `stop_reason=result_budget` at the explicitly requested limit
50. This is a successful bounded query, not a claim of complete tree traversal.
Terminal query `ffbf1a14-36f0-4600-a8db-8520b6c6c428` also succeeds (12 elements,
no truncation). Both omit values; no actions were sent to Finder or Terminal.

A fresh AppKit fixture passed all 21 native UI driver assertions: ordinary text,
checkbox/radio/list selection, secure/readonly/disabled rejection, preconditions,
wait matching/timeout/ambiguity and request deduplication. Its own result confirmed
the expected text, checked/radio, selected index 1 and click count exactly 1.
The fixture exited and the one-shot installer launchd job was removed.
See [native regression evidence](finder-window-fix-native-2026-10-06.json).

| Package | Bytes | SHA-256 |
|---|---:|---|
| macOS ARM debug 1.2.26 | 73381000 | `34b4e7e9c3140662288c6c67c19f84f122ae60e802e36650f23bcf9c546edbd6` |
| macOS Intel debug 1.2.26 | 74287886 | `11a57fe64382f11668e8efd054abb39afc20cc9e1df0dd7630e8982af33e9cb7` |

Both packages were downloaded through native Pixels binary transfer to local
`.build/packages/` and verified. Windows remains on 1.2.24; this fix changes only
the Mac AX backend. Version-allocation tests passed (14 cases).
