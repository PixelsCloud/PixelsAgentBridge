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

The Windows app and Executor service share an installation, but ordinary users
may lack permission to read protected machine credentials. Remote operation
remains available; the local access panel reports that permission limit.
The local status indicator prefers the Executor heartbeat. Older Windows
Executor builds can use a cached, hidden scheduled-task check as a temporary
fallback, so no console window appears. A restricted local WebSocket between
Executor and desktop is planned for authenticated status and events.
