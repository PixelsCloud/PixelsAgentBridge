# Tool acceptance

`python scripts/acceptance.py run isolated` runs protocol/desktop, real stdio MCP,
and Executor regressions. Every run has a fresh UUID and isolated fixtures;
reports and raw test logs remain in `.build/acceptance/<run-id>/` locally.
It never installs packages, restarts a machine, or runs ignored desktop tests.
Run twice after the final changes to check independence.

`tool-coverage.json` maps all 60 registered tools to existing regression sources
and the live suite that must verify them. This is a coverage **plan**, not a claim
that every tool passed. The real rmcp catalog test rejects missing/stale tools
and exports schemas as `catalog.json` for the report.

`run live-basic` is opt-in to the existing two-client transport test. Set
`PAB_TEST_DEVICE_CODE`, `PAB_TEST_PATH` (a disposable/existing fixture), and
`PAB_DATA_DIR` explicitly; control/relay configuration must select the same server
as the installed product. This tests built stdio clients, not recovery of an
installed AI host. Never store device passwords in test arguments or reports.

`live-desktop`, `lifecycle`, and `upgrade` require native `pixels.pab_*` calls and
application/OS observations. The driver reports blocked if asked to automate
these without evidence. Record verified cases with:

```text
python scripts/acceptance.py record live-desktop --device-code 603527578 --case monitor-click --status pass --operation-id UUID --evidence "Fixture received one click; actual coordinates matched"
```

Statuses are pass/fail/skip/blocked/unconfirmed. A transport timeout or disconnect
is unconfirmed, not proof a mutation failed. Query the original request ID;
never automatically submit it with a new ID. Interrupted reports retain the
unconfirmed case and any possible child processes needing cleanup. `record`
stores the supplied observation; it does not independently verify an assertion.

## Monitor input contract

Use xcap 0.8.2 and Enigo 0.5.0 already used by the product. No new OS backend or
virtual-display driver is introduced. The local Enigo Windows patch and public
API references are in `vendor/enigo/PAB-PATCH.md`.

1. Call `pab_list_monitors`, choose its ID, copy that entry's `input_target`.
2. Optionally capture that monitor. `preview_to_desktop` maps returned image
   pixels to native desktop units, including cropped images. Recheck the capture
   geometry against the selected monitor. A screenshot is evidence at capture
   time, not a guarantee the layout/content cannot subsequently change.
3. Call `pab_desktop_input` with `monitor_input: {target, action}`. Action is
   `{type: "move", x, y}` or `{type: "click", x, y, button: "left"}`.
   x/y are nonnegative integer logical coordinates **inside that monitor**.
   For physical_pixels targets, native offset = logical * scale_percent / 100;
   for logical_points targets, native offset = logical. Native origin is not
   scaled. Use the returned coordinate space, never the caller's own OS/DPI.
4. Request IDs deduplicate monitor mutations. This requires Executor system v8
   and helper v3. Existing legacy events and window batches keep their semantics.

Example: Windows monitor origin (-1920, 0), scale 150%, logical point (100, 200)
maps to native desktop (-1770, 300). A Retina screenshot's 200 image pixels may
map to 100 native points; do not send 200 directly as logical input.

The target includes helper instance, geometry, scaling and rotation. Removed
targets, changes, stale helper instances, and out-of-range coordinates fail;
the code never silently falls back to primary. ID reuse with identical geometry
cannot prove physical identity across hotplug. The pointer is observed before
click; application effects still require separate verification. Input is not an
OS-atomic transaction against physical mouse input or changes by other programs.
