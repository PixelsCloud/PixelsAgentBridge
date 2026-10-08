# Desktop packages

On macOS, run `bash packaging/desktop/build-macos.sh debug all` after installing
frontend dependencies. This delegates to `scripts/build.py macos --macos-arch all
--package`: both architectures share one version allocation, while tar/PKG creation
does not allocate another version. The wrapper selects Python 3.12+ and Homebrew
tools for non-interactive builds. See [MACOS.md](../../MACOS.md).

## macOS

On a Mac with Rust, Xcode Command Line Tools, Node/npm and Python 3.9+, run
`bash packaging/desktop/build-macos.sh release` from the repository root
(`debug` is also supported). The native-architecture archive in `.build/packages`
contains the ad-hoc signed `Pixels Agent Bridge.app`, MCP, Executor, dedicated
macOS scripts and launchd plists. It is not Developer-ID signed or notarized.
The adjacent SHA-256 manifest records the complete archive checksum.
The macOS wrapper uses the unified build entry to allocate one installer release version.
Rust, npm, Tauri and the built app retain their internal versions. macOS tar/PKG
metadata uses the verified build record's release version independently of the app version.

Unpack the complete archive and, from a desktop user account, run
`sudo bash install.sh WSS_CONTROL_URL HTTPS_RELAY_URL`.
Open the app from Applications and grant Screen Recording and Accessibility as
needed. Quit Desktop/MCP clients before upgrading with another complete archive.
Uninstallation preserves machine and user data. See [MACOS.md](../../MACOS.md)
for exact paths, permissions, account semantics and verification limitations.
Windows/Linux use their existing scripts unchanged.

### Double-click macOS installer

`build-macos.sh release aarch64` and `build-macos.sh release x86_64` build
architecture-specific archives on a Mac (install the corresponding Rust target
first). To wrap either verified archive in a native Installer `.pkg`, use Python
3.12+. The current protocol and installers no longer require a deployment UUID:

```sh
python3.13 packaging/desktop/build_macos_pkg.py --arch aarch64
python3.13 packaging/desktop/build_macos_pkg.py --arch x86_64
```

The default control/relay addresses match `build_nsis.py`:
`wss://pab.rgaa.vip/control` and `https://pab-relay.rgaa.vip`. Override them with
`--control-url` and `--relay-url` for another server. No UUID is embedded or requested.

Each `*-setup.pkg` embeds all binaries and installation scripts, requires admin
authorization, checks the native CPU architecture, installs only on the running
system volume, and grants local access to the active desktop user. No Terminal
commands or deployment inputs are needed on the target Mac. Quit Desktop/MCP
clients first. Screen Recording and Accessibility still require manual consent.
The package reuses `macos/install.sh` in a scripts-only component; its receipt is
not a file inventory. Use the installed `uninstall.sh` to uninstall; existing
data is retained. Installer failures do not provide automatic rollback.

Pass `--sign 'Developer ID Installer: ...'` to sign the package if that identity
is available. App signing and notarization are separate; an unsigned package
is not a notarized public release. The per-architecture `SHA256-macos-*-setup-*.json`
records the package hash and deployment settings. Building never installs PAB.

Run `python3.13 packaging/desktop/test_macos_pkg.py` for validation tests; set
`PAB_PKG_TEST_ARCHIVE` to an existing macOS archive to additionally build and
expand a temporary fixture installer without running its installation scripts.

## Windows and Linux

During development, use `python scripts/build.py desktop --package` from the
repository root after `npm ci` in `apps/desktop`. The unified entry point defaults
to Debug, increments the product version once, and builds all three components
before packaging. See [BUILDING.md](../../BUILDING.md). Archive names and checksum manifests distinguish
the profiles so test and release packages cannot silently replace one another.
The Windows ZIP includes `INSTALL-WINDOWS.txt` with the PowerShell installation
steps. To make a single EXE installer for another Windows computer, copy NSIS
3.12 into `tools/nsis` and run
`python packaging/desktop/build_nsis.py`
after building the complete Windows Debug ZIP. The NSIS builder checks the ZIP
checksum and every Debug binary before embedding them. The resulting
`.build/packages/pixels-agent-bridge-windows-x86_64-debug-<version>-setup.exe` has the CN
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

For an explicitly requested Release build, use
`python scripts/build.py desktop --profile release --package`. The resulting Windows archive contains
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

On a Windows host, build the Linux archive with
`python scripts/build.py linux --package`. The existing
`powershell -File packaging/desktop/build-linux.ps1` delegates to that entry point.
It keeps Cargo downloads and compilation output in `.build` on the workspace drive
and creates the Debug tarball. Use `--profile release` only for a release build.
The Linux archive contains Executor, MCP and service/install scripts; it does
not include Tauri, a desktop helper or XDG autostart. No GTK/WebKit/display stack
is required. Run `install.sh` as root with the control URL and Relay URL.
Service and selected-user commands, PTYs, Git, files and transfers work without
graphical login. Desktop queries, screenshots and input return unsupported;
this is the current product scope, not a missing permission to repair.
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
