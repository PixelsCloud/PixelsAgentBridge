# Build concurrency

The user requires compilation to use at most 1 core per machine. On each
machine, use no more than 1 concurrent build job across all sessions, including
direct Cargo commands and nested builds. Set Cargo, CMake and other nested
build concurrency to 1; do not overlap compilation commands on the same machine.
Do not update the local installed Pixels application unless necessary: other
sessions use it.

# Configuration format

All application configuration formats owned by this project must use TOML on
Windows, macOS and Linux, including installer defaults, Desktop preferences and
MCP settings. Preserve user configuration on upgrades and migrate legacy JSON/env
configuration without executing it. Third-party configuration contracts and
runtime data (credentials, databases, protocol messages, release manifests) keep
their required formats.
