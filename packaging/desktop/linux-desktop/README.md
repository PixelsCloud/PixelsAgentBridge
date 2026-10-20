# Linux native desktop package

This is the graphical React/Tauri client and system Executor for Ubuntu 22.04
x86_64. Its GTK/WebKit runtime baseline is separate from the existing glibc 2.31
headless package. A Wayland desktop can run the client; controlling this computer's
Wayland desktop remains subject to the existing screenshot/input limitations.

## Build

Run from the repository root on a host with Docker and npm:

```sh
python scripts/build_linux_desktop.py --version 1.0.0 --package \
  --headless-package /absolute/path/to/verified-headless-release.tar.gz \
  --headless-sha256 SHA256_FROM_THE_VERIFIED_RELEASE
```

The GUI builds on an isolated Ubuntu 22.04 sysroot with Cargo 8 + CMake 2 and a
10-CPU Docker limit. It uses its own target volume, so it does not change the
headless build cache or Windows installation. Frontend and native GUI use the
same explicit desktop installer version; `build-version.json` is not modified.
The verified headless archive supplies unchanged Executor, MCP, document Worker
and offline PDF/OCR assets. The packager rejects checksum, architecture and
archive traversal/link mismatches before assembling a package.

Artifacts and build receipts are under `.build/linux-desktop/VERSION/`.
Repackaging verifies the GUI's adjacent `build.json` against the requested version,
architecture and ELF hash; missing receipts and modified binaries are rejected.

## Install and use

```sh
sudo apt install ./pixels-agent-bridge-desktop-linux-x86_64-1.0.0.deb
```

Open **Pixels Agent Bridge** from the application menu, or run
`pixels-agent-bridge` as the normal signed-in user. The GUI connects to the
installed local Executor, shows the device code/password, and provides the same
account, saved devices, remote-terminal and settings interface as Windows/macOS.
Do not run the graphical app with sudo.

On VMware, the launcher automatically disables WebKitGTK's DMABUF presentation
path to avoid the black blocks and broken SVG rendering observed on Ubuntu 22.04
Wayland. Other machines keep their default graphics path; an explicitly provided
`WEBKIT_DISABLE_DMABUF_RENDERER` value is respected.

The package enables the system Executor service and adds a login autostart entry
for the GUI, showing its main window after sign-in. Closing the main window keeps it in the tray;
launching the application again restores the existing window. The tray's Exit
action exits the GUI while keeping the system Executor running.

The desktop package also enables `pixels-agent-bridge-media.service`. Its root
supervisor observes the active local console session and starts a separate media
helper as that user or login-screen account. It supplies a private temporary local
access file and the verified graphical environment, then reaps the old helper on
session changes. Closing the GUI does not stop this service. This lifecycle support
does not by itself mean that every compositor permits lock-screen capture; actual
X11/Wayland login support is tracked in the remote-desktop acceptance records.

The service preserves the existing command/terminal account selection. It does
not enable automatic login or change the display manager's configuration.

Installation grants local service access to the installing/active desktop user.
To enable another local account:

```sh
sudo /opt/pixels-agent-bridge/grant-desktop-user.sh USER
```

The helper resolves that user's data directory through the installed program,
respects its TOML configuration and writes the private access file as that user.
It does not grant access to root's account credentials or endpoint database.

## Upgrade and remove

Close the GUI before installing a newer desktop deb. The package preserves
`/etc/pixels-agent-bridge/config.toml`, `/var/lib/pixels-agent-bridge` device data
and each user's existing account/settings/history directory. Project-owned
configuration stays TOML; desktop entries and Debian metadata follow the OS format.
The GUI does not query or install Linux headless releases as GUI updates.

```sh
sudo apt remove pixels-agent-bridge
```

Removal stops installed processes and removes the application/menu/autostart
entries. Device data and user configuration are retained for reinstallation.
No update to another machine or another platform is part of this package.

## Publish through the website

The website uses the independent `linux-desktop` release target. The existing
`linux` target and one-line headless installer keep their current artifacts.

```sh
python scripts/release.py linux-desktop \
  --headless-package /absolute/path/to/verified-headless-release.tar.gz \
  --headless-sha256 VERIFIED_SHA256 \
  --notes-zh "Linux 桌面版发布说明" --notes-en "Linux desktop release notes"
```

Configure `PAB_RELEASE_API_TOKEN` and `PAB_RELEASE_PRIVATE_KEY_FILE` privately,
as for the existing platform releases. The script reserves an independent
version, builds and checks the GUI/package receipts, signs, uploads to COS,
and finalizes through the website. Use `--resume` to continue an interrupted
release. The first public desktop release is V1.0.1; its complete website download
was installed over V1.0.0 on the Ubuntu VM with identity and settings preserved.
