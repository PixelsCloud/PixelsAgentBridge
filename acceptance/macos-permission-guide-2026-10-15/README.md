# macOS permission settings acceptance — 2026-10-15

The settings page lists two Pixels programs and four permissions. Granted Screen
Recording and Accessibility show **Normal** without an application button.
Denied permissions have **Request access**. Full Disk Access is explicitly
**Unconfirmed**: there is no reliable public status API, so opening settings,
copying a path, or closing the guide never marks it granted.

The background service is labelled `pab-executor` and explicitly identified as a
Pixels program. Both programs have separate Full Disk Access request buttons.
Request access opens System Settings directly, without Finder or an in-app
dialog. Program paths, optional Finder actions, repair advice and restart controls
are available only through the program's More options → Program not listed in
System Settings? menu. Main settings contain no long instructions.

## Validation

- Final compact-page browser suite: 14 passed, including actual-observation versus
  request, no automatic Finder or dialog on request, optional exact program
  routing, path copying, persistent native errors, separate
  restart confirmations, missing executable, Windows exclusion, and six
  locale/theme layout combinations at 950 × 640 without overflow.
- TypeScript and production frontend build passed.
- Mac ARM64 native permission unit tests: 2 passed. Windows native check passed.
- Initial native Mac validation after the user enabled permissions: screenshots
  and window control worked; app displayed both Screen Recording and
  Accessibility available. Both program path copy buttons were verified against
  the GUI session pasteboard (only equality was reported, no clipboard contents).
  Finder selected the installed app. Final compact-page native verification is
  recorded in `results.json`.

## Behaviour and scope

- Native commands allow only known privacy panes and the two fixed Pixels
  program targets. Path/reveal/service-restart commands require the main window.
- Screen and Accessibility requests use the app's native permission prompts.
  Full Disk Access opens its privacy pane directly. Only an explicit Reveal in
  Finder action in the optional missing-program guide selects a program.
  The user completes macOS switches and administrator confirmation.
- Permission polling is read-only; it never prompts or clears action errors.
- Restart controls are inside collapsed repair help and require confirmation.
  Cancelling the confirmation has no effect. Executor restart additionally uses
  macOS administrator confirmation and can interrupt remote work.
- No TCC database reads/writes, signature changes, or local Windows installation.
  Persistent local Mac signing identity is retained. Product version V1.2.99;
  existing internal app bundle version 1.2.49 remains unchanged.
- macOS system switches remain authoritative for Full Disk Access. A program
  missing from disk disables its request rather than granting another executable.

The screenshots `guide-light.png` and `guide-dark.png` show the final compact
browser fixture. Native screenshots, installer identity/file verification, and
package digest are included alongside this record after installation.
