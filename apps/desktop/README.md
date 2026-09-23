# Pixels Agent Bridge desktop

This is the single Tauri 2 + React desktop application. One window can show
this computer's access code and temporary password, approve ownership, connect
to other devices, and run native remote commands on multiple connections.
The Executor remains a background process from the same installation.

The interface supports Simplified Chinese and English. It starts with the
system language and saves a manual choice locally. The window uses a custom
Tauri titlebar and separate overview, remote, activity, and ownership views;
the main window has no visible scrollbars. Remote command results update
while tasks run. The app does not open a local HTTP interface.

Run `npm ci` and `npm run tauri build -- --no-bundle` on Windows. The Rust
project under `src-tauri` is kept outside the core Cargo workspace, so server,
Relay, Bridge, and Executor builds do not compile Tauri.

The Executor serves an authenticated WebSocket on `127.0.0.1:7843` for local
device status, temporary password changes, and ownership approval. The desktop
keeps the connection open, updates the window from status events, and retries
every three seconds after a disconnect. The token stays in the machine data
directory and in the installing user's private data directory; the UI does not
read the machine's endpoint key. The installer grants this access to its
interactive user. `PAB_LOCAL_IPC_PORT` can select another local port if needed.

Older Executor builds without this WebSocket remain usable through the
privileged local data path. Their Windows scheduled-task status check is cached
and hidden, so it does not flash a console window.
