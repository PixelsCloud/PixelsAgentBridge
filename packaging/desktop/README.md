# Desktop packages

During development, build Debug binaries and package them with
`python packaging/desktop/build.py --platform windows --profile debug`.
From `apps/desktop` on Windows, build the Debug desktop executable with
`.\node_modules\.bin\tauri.cmd build --debug --no-bundle`. The explicit CLI
call matters: `npm run tauri build -- --debug --no-bundle` did not forward the
flags in our PowerShell test and produced a Release executable.
The default profile is Release. Archive names and checksum manifests distinguish
the profiles so test and release packages cannot silently replace one another.
The Windows ZIP includes `INSTALL-WINDOWS.txt` with the PowerShell installation
steps. To make a single EXE installer for another Windows computer, copy NSIS
3.12 into `tools/nsis` and run
`python packaging/desktop/build_nsis.py`
after building the complete Windows Debug ZIP. The NSIS builder checks the ZIP
checksum and every Debug binary before embedding them. The resulting
`.build/packages/pixels-agent-bridge-windows-x86_64-debug-setup.exe` has the CN
WSS control and HTTPS Relay URLs by default; use `--control-url` and
`--relay-url` to target another deployment. It uses the same `install.ps1` and
`uninstall.ps1` as the ZIP, creates an all-users Start menu entry, and appears
in Windows Installed Apps. It asks before enabling service-generated
Ctrl+Alt+Delete when the machine policy does not already allow it. This Debug
installer is not code-signed.
For Windows integration testing, always deploy the complete Debug archive and
run its `install.ps1`. Verify the archive checksum before installation, then
check the installed service, session supervisor, and desktop app together.
Do not copy an individual executable or script into an existing installation
for debugging. A code change requires rebuilding and deploying a new complete
Debug archive. Keep test data during installation unless a test explicitly
requires resetting it.

Build the Windows core binaries with
`cargo build --locked --release -p pab-executor --bin pab-executor -p pab-bridge --bin pab-mcp`. In
`apps/desktop`, run `npm ci` and `.\node_modules\.bin\tauri.cmd build --no-bundle`, then run
`python packaging/desktop/build.py --platform windows`. The resulting Windows archive contains
one Tauri app, the background Executor, the MCP stdio tool, and installation
scripts. The adjacent SHA-256 manifest verifies the archive. Release profiles
strip symbols.
The standalone `pab-bridge` CLI is not included in client packages. The
desktop and MCP binaries still compile and use the `pab-bridge` Rust library.
On Windows, install the complete package and open Settings > AI Agent in the
desktop app. If Codex CLI is installed for the current user, Enable registers
the bundled `pab-mcp.exe` with Codex, checks that its tools start, and sets the
Pixels MCP server's tool approval mode to `approve`. It removes Pixels-only tool
allow/deny lists and per-tool approval restrictions so commands and other
device operations can run without a Codex approval prompt. `auto` can still
request approval based on tool annotations; `approve` pre-approves tool calls. Restart
Codex after enabling. To use a remote device for the first time, connect to it
once on the desktop Remote page with its device code and password. The desktop
stores that credential in the user's Bridge SQLite database; the bundled MCP
process reads the same database, including after the desktop window closes.
The Settings switch changes only the current user's Codex configuration.
Disabling removes only the Pixels MCP entry. Other Codex MCP entries are left
untouched. On Linux and macOS, register the installed `run-mcp.sh` manually so
the same deployment settings used by the desktop are loaded before MCP starts.

Run `install.ps1` from an elevated PowerShell window with the
WSS control URL, and HTTPS Relay URL. Installation starts the Executor as a
LocalSystem Windows service and installs `run-app.ps1` as the single interface
entry point. Windows policy `SoftwareSASGeneration` must allow services (value
1 or 3); the installer checks this before replacing an existing installation.
Run the installer again from an unpacked newer archive to upgrade. It stops
the installed Executor and desktop processes before replacing their binaries,
then restarts the Executor. Reopen the desktop with `run-app.ps1` afterward.
That window can both receive and make connections. Device identity, passwords,
and local task history are retained when `uninstall.ps1` removes the program.
Running the desktop executable without installing the local Executor remains an
operator-only setup. Its overview shows that the local device is not ready to
receive connections while remote connections remain available.
The remote view can run commands and transfer binary files in either direction.
File transfer shows live progress, supports cancellation and retry from a partial
file, and replaces an existing destination only when explicitly requested.
The installer gives its current Windows user a private local WebSocket token.
Other local users require a separate privileged `pab-executor issue-local-access
<user-token-file>` operation before they can view this device's password from
the desktop window. Device claims and ownership approval have been removed;
server administrators manage devices directly in the Web console.
The eight-character device password remains valid across ordinary Executor
restarts and package upgrades. Use the explicit `pab-executor rotate-password`
command when rotation is needed; reconnecting operators then need the new
password.
The machine's `executor.sqlite3` stores its device code, current temporary
password, password hash, and task history together. A registered device can
display its saved access information while the control server is unavailable.

After building the frontend, build the Linux archive with
`docker build -f packaging/desktop/Dockerfile.linux --output type=local,dest=.build/guest-desktop-linux-release .`.
For a Debug package, add `--build-arg PAB_PROFILE=debug` and use a separate
`guest-desktop-linux-debug` output directory, then run
`python packaging/desktop/build.py --platform linux --profile debug`.
On a Windows host with limited C: space, use
`powershell -File packaging/desktop/build-linux.ps1` instead. It keeps Cargo
downloads and compilation output in `.build` on the workspace drive and creates
the Debug tarball. Pass `-Profile release` only for a release build.
The archive includes the same Tauri app and background components.
Run `install.sh` as root with the control URL and Relay URL, then launch
`run-app.sh` as the interactive user. The old browser UI is absent from both
platforms. On Linux, installation registers a hidden XDG autostart entry for
the session helper. It captures the desktop in the logged-in graphical session;
mouse and keyboard control currently require X11. A Wayland session returns an
explicit unsupported-input error. Linux graphical control and pre-login
capture still require testing on a graphical Linux machine.
Headless Linux can run the Executor, but opening the desktop requires the
distribution's GTK 3, WebKit2GTK 4.1, JavaScriptCoreGTK 4.1, libsoup 3, GBM,
and display-session libraries. The SG headless host does not have these desktop
runtime libraries; its real package install and upgrade have validated the
background Executor, not the graphical application.
Re-running `install.sh` stops the installed service and processes before
replacing binaries, restarts the service, and preserves data under
`/var/lib/pixels-agent-bridge` and the user's local data directory.
On Linux, installing with `sudo` grants local WebSocket access to `SUDO_USER`;
installing directly as root grants it to root.

Each process writes rotating logs under its persistent data directory. The
active file plus four archives are limited to 16 MiB each.

## Unattended Windows access

The Windows installer starts the machine Executor as a LocalSystem service and
the session supervisor as a SYSTEM startup task. The supervisor starts a headless helper in
the active Windows session. A user does not need to sign in to the OS or open
the Tauri window before another operator connects. On Windows 90, command and
file access plus a Winlogon screenshot were verified after the interactive
user logged out. Keyboard and mouse input were verified on the signed-in desktop.
The LocalSystem service's secure attention request was verified on Windows 90:
it advances the Ctrl+Alt+Delete lock screen to the password prompt, where
ordinary remote keyboard input also works. A full Windows sign-in was not tested.

The session supervisor task runs without an execution time limit and restarts
after a failure. The visible Tauri window remains the single user interface
and can be opened when a user logs in. Uninstallation removes the service and
supervisor task while retaining machine identity and task data.
# Windows installer behavior

The NSIS installer starts the desktop window after installing the background
service. The window is launched through the signed-in Explorer shell so it
uses the interactive user's session. The same shortcut remains available in
the Start menu. The Settings page contains the interface language and a
user-scoped operator server override; saving a server change requires an app
restart. The machine service keeps its installation-time server configuration.
