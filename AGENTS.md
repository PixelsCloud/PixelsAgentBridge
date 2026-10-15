# Build concurrency

The user requires all compilation to use at most 10 cores. Use no more than
10 concurrent build jobs, including direct Cargo commands and nested builds.
Do not run concurrent compilation commands whose combined jobs exceed 10.
Do not update the local installed Pixels application unless necessary: other
sessions use it.

# Configuration format

All application configuration formats owned by this project must use TOML on
Windows, macOS and Linux, including installer defaults, Desktop preferences and
MCP settings. Preserve user configuration on upgrades and migrate legacy JSON/env
configuration without executing it. Third-party configuration contracts and
runtime data (credentials, databases, protocol messages, release manifests) keep
their required formats.
