# Pixels Agent Bridge desktop

The interactive desktop UI uses Tauri 2, React and TypeScript. The Executor
continues to run as a separate background service. This first window reads the
local machine's device code, current temporary password, and Executor heartbeat;
it can approve a claim ID entered by the device administrator.
The interface supports Simplified Chinese and English. It uses the system
language on first launch and saves a manual choice locally.

Run `npm ci` followed by `npm run build` to validate the frontend. Run
`npm run tauri build` to build the desktop application on Windows. The Rust
project under `src-tauri` is kept outside the core Cargo workspace to avoid
compiling Tauri when building the server, Relay, Bridge or Executor.

The window must run with permission to read the machine data directory. The
device page does not open a local HTTP listener. The operator pages will be
ported into this app next; the current `pab-mcp --ui` browser page remains a
temporary test interface.
