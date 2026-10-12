# Offline browser runtime

The Desktop installer bundles Node.js 22.15.0 and `@playwright/mcp` 0.0.83. Target machines do not run npm or need Chrome Web Store access. The Node executable or archive is verified against the [official release checksums](https://nodejs.org/download/release/v22.15.0/SHASUMS256.txt) before packaging. The build manifest records SHA-256 for every bundled file.

On the target OS, run `python browser-runtime/build.py`. The script downloads the pinned Node build if needed and runs `npm ci --ignore-scripts` only on the build machine. It creates `.build/browser-runtime/browser-runtime-<platform>.zip` or `.tar.gz`. To build Windows from a previously downloaded official `node.exe`, pass `--node <path>`; the pinned checksum is still required. Build the Desktop archive after this command succeeds.

Pixels launches its own Chrome window, or its own Edge window if Chrome is absent. It passes an account-specific `--user-data-dir` to Playwright MCP, keeping website login state between sessions. This profile is under the target OS user's Pixels data directory, separate from the everyday Chrome/Edge profile and from other Pixels accounts. No browser extension is used or packaged. If both browsers are missing, the task reports `browser_missing`.

The browser tools bind a session to the signed-in Pixels user, initiating MCP operator and selected OS user. They do not expose the upstream `browser_evaluate` or `browser_run_code_unsafe` tools. Browser downloads are reported only when Playwright associates them with the current session.

The bundled `NODE-LICENSE.txt` and `PLAYWRIGHT-LICENSE.txt` accompany redistributed components. Verify new upstream versions, licenses, hashes and the Windows/macOS smoke fixture before changing any pin.
