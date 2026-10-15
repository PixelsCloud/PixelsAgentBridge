# macOS file access status correction

This supersedes the fixed “Unconfirmed” disk rows in the earlier permission
guide. The two program cards now show **File access**, an observed check of
Downloads, Documents and Desktop. It is deliberately not a claimed observation
of the macOS Full Disk Access switch. That switch remains in System Settings.

## Implementation

- Desktop probes are children of the running app executable, in its account and
  bootstrap context. Executor probes are children of the running daemon. They
  check the same user's three folders independently.
- A new, dedicated authenticated local IPC socket asks the daemon to check. The
  existing native PID/UID challenge proves the actual client identity before the
  server resolves its home. The request accepts no home path, account, command or
  executable selection from the UI. Existing helper event streams are unchanged.
- Each child checks exactly one allowlisted folder with `read_dir` plus at most
  one iterator entry. No filenames or file contents are returned. No files are
  created or changed and no TCC database is opened.
- Each folder worker has a 2-second wait deadline, followed by kill and a bounded
  reap attempt. Output is bounded to 4 KiB. A retained worker slot prevents repeat
  checks from creating more children while an unreaped child is still alive.
  The supervisor survives caller cancellation and cleans up its own children.
- All three checks must succeed for **Normal**. Permission denial is
  **Access restricted**; missing directory, timeout, crash, unavailable service or
  invalid reply is **Check failed**, with details in the tooltip. A partial or
  failed check never reports Normal.
- The settings page checks both programs on entry and explicit refresh. Returning
  from an explicitly opened privacy pane rechecks the relevant program. A return
  during an existing check queues one fresh check rather than losing the update.
- The settings buttons open the shared macOS privacy pane and explicitly name
  which Pixels program to enable. They never select or grant a different
  program, show Finder automatically, or treat opening settings as a grant.
  Programs not in the system list have an optional, correctly targeted add guide.

## Validation

- Browser suite: 18 passed, including separate program observations, normal to
  denied transition, post-authorization return, timeout/error handling, responsive
  UI during a pending check, and locale/theme/layout regressions.
- Windows shared-module unit tests: 2 passed.
- Mac shared-module unit tests: 5 passed, including worker timeout, crash,
  malformed/oversized reply, and caller cancellation/concurrency bounds.
- Mac permission command tests: 2 passed.
- Final installer and native device evidence is recorded in `results.json`.
- Mac 419438177: the already-authorized executor immediately reported Normal.
  The main app initially timed out while macOS displayed its Downloads consent
  prompt. All probe workers were reaped and the executor remained responsive.
  After the user added/enabled the main app in Full Disk Access, both programs
  reported Normal. An explicit fresh refresh also reported Normal for both;
  Screen Recording and Accessibility remained Normal. Both code signing
  requirements, device identity and all installer manifest files were verified.

The denied/revoked state transitions in UI tests are controlled observations.
Native POSIX access checks use an isolated fixture; live user TCC grants are not
revoked to simulate failure. No other user's files are modified.
