# Desktop Release packages

Build the three core binaries for each target with `cargo build --locked --release`.
For the Windows local device window, run `npm ci` and
`npm run tauri build -- --no-bundle` in `apps/desktop`, then run
`python packaging/desktop/build.py`.
The release profiles strip symbols.
The packager uses Windows binaries in `target/release` and Linux binaries in
`.build/guest-desktop-linux-release` by default. Build the Linux binaries with
`docker build -f packaging/desktop/Dockerfile.linux --output
type=local,dest=.build/guest-desktop-linux-release .` when building on Windows.
It creates archives and a SHA-256 manifest in `.build/packages`.

To package macOS after a native or CI build, pass `--macos-bin-dir PATH` with
the three macOS Release binaries. This path is prepared but has not been tested
on a Mac.

For an operator, extract the archive and run `install.ps1` on Windows or
`install.sh operator` on Linux/macOS, providing the deployment ID, WSS control
URL, and HTTPS relay URL. Launch `run-ui.ps1` or `run-ui.sh` from the installed
directory. The terminal shows the local UI URL. Enter the 9-digit code and the
temporary password shown on the remote device. Login is not required.

For an Executor, install with `-Executor` on Windows or `install.sh executor`
as root on Unix. It starts as a scheduled task, systemd service, or launchd
daemon. On Windows, open an elevated PowerShell window and run
`run-device-ui.ps1` from the installed directory. This Tauri/React window shows
the current 9-digit device code and temporary password and can approve a claim
request. The administrator can also read the code and password using
`pab-executor show-access` with `PAB_DATA_DIR` set to the machine data directory.
The password rotates when the Executor process starts again. The Linux local
device window is still awaiting a native Linux Tauri build; the command-line
access display remains available there.

`uninstall.ps1` and `uninstall.sh` remove application files and service
registration. They leave the user and machine data directories untouched, so
reinstalling retains device identity and local task history.

Each process writes rotating logs under the persistent data directory's `logs`
subdirectory. The active file plus four archives are limited to 16 MiB each.
Set `PAB_LOG_DIR` to override the directory and `PAB_LOG_LEVEL` to adjust the
filter. The temporary device password is never written to the log.
