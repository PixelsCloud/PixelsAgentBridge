# Desktop Debug packages

Build the three binaries for each target with Cargo's Debug profile, then run
`python packaging/desktop/build.py`. The packager uses the Windows binaries in
`target/debug` and Linux binaries in `.build/guest-desktop-linux` by default.
It creates archives and a SHA-256 manifest in `.build/packages`.

To package macOS after a native or CI build, pass `--macos-bin-dir PATH` with
the three macOS Debug binaries. This path is prepared but has not been tested
on a Mac.

For an operator, extract the archive and run `install.ps1` on Windows or
`install.sh operator` on Linux/macOS, providing the deployment ID, WSS control
URL, and HTTPS relay URL. Launch `run-ui.ps1` or `run-ui.sh` from the installed
directory. The terminal shows the local UI URL. Enter the 9-digit code and the
temporary password shown on the remote device. Login is not required.

For an Executor, install with `-Executor` on Windows or `install.sh executor`
as root on Unix. It starts as a scheduled task, systemd service, or launchd
daemon. The administrator can read the current 9-digit code and temporary
password using `pab-executor show-access` with `PAB_DATA_DIR` set to the machine
data directory. The password rotates when the Executor process starts again.

`uninstall.ps1` and `uninstall.sh` remove application files and service
registration. They leave the user and machine data directories untouched, so
reinstalling retains device identity and local task history.
