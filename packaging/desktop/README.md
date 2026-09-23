# Desktop Release packages

Build the Windows core binaries with `cargo build --locked --release`. In
`apps/desktop`, run `npm ci` and `npm run tauri build -- --no-bundle`, then run
`python packaging/desktop/build.py`. The resulting Windows archive contains
one Tauri app, the background Executor, the MCP stdio tool, installation
scripts, and a SHA-256 manifest. Release profiles strip symbols.

Run `install.ps1` from an elevated PowerShell window with the deployment ID,
WSS control URL, and HTTPS Relay URL. Installation starts the Executor as a
scheduled task and installs `run-app.ps1` as the single interface entry point.
That window can both receive and make connections. Device identity, passwords,
and local task history are retained when `uninstall.ps1` removes the program.

After building the frontend, build the Linux archive with
`docker build -f packaging/desktop/Dockerfile.linux --output type=local,dest=.build/guest-desktop-linux-release .`.
The archive includes the same Tauri app and background components.
Run `install.sh` as root with the same three connection settings, then launch
`run-app.sh` as the interactive user. The old browser UI is absent from both
platforms. The Linux application has been compiled and packaged, but a Linux
graphical session has not yet been available for interactive verification.

Each process writes rotating logs under its persistent data directory. The
active file plus four archives are limited to 16 MiB each.
