# Windows installer: MCP respawn during upgrade

The local 1.2.52 installer failed at `Cannot replace running program: ...\pab-mcp.exe`.
The remaining installed MCP was launched by a Node process belonging to DeepSeek
Harness. The previous installer killed matching processes once, slept two seconds,
then rejected any remaining process. A client restarting its MCP during that gap
could reopen the old executable and make the installer abort.

## Change

- Stage the three replacement binaries before modifying installed files.
- Rename existing binaries to unique temporary paths before terminating their
  processes, preventing clients from reopening the old executable at its launch
  path. Retry retirement for up to 30 seconds if the file cannot yet be renamed.
- Terminate only verified full executable paths from this installation; do not
  terminate the AI Agent client or another installation with the same filename.
- Publish complete staged binaries by rename. If replacement fails, restore the
  previous files; preserve backups when restoration itself fails.
- NSIS compression temporary files now use the repository staging directory.
  During this task the system drive had only about 200 MB free, which caused the
  first NSIS compression attempt to fail creating a memory-mapped temporary file.

## Verification

`powershell.exe -NoProfile -ExecutionPolicy Bypass -File packaging/desktop/windows/test-install-files.ps1`

Passed with real Windows executable fixtures under an isolated `.build` directory:

1. A watchdog immediately restarts a terminated MCP fixture, reproducing the old
   installer's race. The new installer replaces its bytes successfully and stops
   the old process while leaving another directory's same-name process running.
2. An injected failure after the first binary is published restores all three
   original binary hashes.

`python -m unittest discover -s packaging/desktop -p test_packaging.py` passed.

## Package

- Installer: `pixels-agent-bridge-windows-x86_64-release-1.2.54-setup.exe`.
- Size: 23,393,900 bytes.
- SHA-256: `1863b10d93e3d143b20e43119444dd619fc2a287e25f61b03d2b4d9ea624fcae`.
- Reuses the verified 1.2.52 package's three compiled binaries unchanged. Only
  installer scripts and installer version changed; no Cargo or frontend build.
- Installer file/product version, payload script, archive CRC and binary hashes
  verified. NSIS retry completed using disk D for temporary files.
- No installed client was killed or replaced during these tests. Full installation
  on the user's actual installation has not been run by this task.
