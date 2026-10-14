# Disposable Desktop terminal feasibility proofs

These standalone projects do not modify or replace the stable command, MCP
terminal, embedded Desktop terminal, or their protocols and history storage.
They are development fixtures, not a shipped service or finished feature.

## Scope

- server.rs directly uses portable-pty 0.9.0 in a dedicated process. It opens
  persistent Windows PowerShell with a selected native identity, a 256KiB
  consumption credit budget, independent input and administration listeners.
- probe.py creates three forwards through one installed Pixels MCP's existing
  authenticated PAB connection. Each TCP client uses a separate QUIC stream.
  It checks state retention, precise byte hashes, backpressure, Ctrl+C, resizing,
  concurrent stable commands/MCP terminal operations, and owned worker shutdown.
  This TCP wrapper proves primitives, not the planned DesktopTerminal protocol.
- webview/ runs a separate native Tauri/WebView2/xterm window on the target.
  It checks its real IPC Channel, bounded aggregation, rendering ACKs, window
  hide/recovery, two output waves, bounded scrollback, and memory samples.
  Its sockets are local on the target. The finished feature must integrate and
  re-test the two paths over PAB, including Relay.

The server listens only on 127.0.0.1 and expires after five minutes. Never run
it as a public service. It owns its test Shell and exits its worker after
shutdown, so a blocked ConPTY reader cannot trap the installed Executor.
Inputs are synthetic; PSReadLine persistence is disabled inside the fixture
Shell. No user profile scripts or system runtime registration are edited.

## Reproduce

Build with at most 18 compiler jobs:

    cargo build --release --manifest-path acceptance/prototypes/desktop-terminal/Cargo.toml --target-dir target -j 18

Using the normal Pixels upload tools, put terminal-proof-server.exe in
C:\Users\Administrator\Downloads\PixelsTerminalProof-20261014 on the authorised
test device. The driver requires a configured/logged-in MCP and a target with
port forwarding. It never receives credentials as arguments. Use an unused
starting port and a fresh output directory each time:

    python acceptance/prototypes/desktop-terminal/probe.py --mcp target/release/pab-mcp.exe --port 39161 --mode user --result .build/acceptance/terminal-user.json

For the window proof, use the existing Node dependencies:

    node apps/desktop/node_modules/vite/bin/vite.js build --config acceptance/prototypes/desktop-terminal/webview/vite.config.ts
    cargo build --release --manifest-path acceptance/prototypes/desktop-terminal/webview/Cargo.toml --target-dir apps/desktop/src-tauri/target -j 18

Start a dedicated server on 39081/39082/39083 as the target user, then launch
the window executable with the existing app tool in that user's desktop.
The Windows fixture uses the existing runtime folder
C:\Program Files (x86)\Microsoft\EdgeWebView\Application\154.0.4258.62 only for its
own process. Inspect/change this fixture path on another machine. Its identifier
is vip.rgaa.pixels.terminal-proof. It writes progress/result beside the executable,
closes its server, and exits automatically.

Outputs and generated icons/schemas are ignored. The icon is copied from the
existing application during build. This is not a framework upgrade: its lock
retains the installed Desktop dependency versions.

Check success, produced/consumed/acked counts, SHA-256, credit maximum, and
cleanup. Shell exit alone did not produce ConPTY EOF on test Windows; dedicated
worker shutdown did. A timeout is never a pass. Longer and cross-platform
acceptance remain part of implementing the finished feature.
