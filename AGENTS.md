# Build concurrency

The user requires compilation to use at most 10 cores per machine. On each
machine, use no more than 10 concurrent build jobs across all sessions, including
direct Cargo commands and nested builds. Account for nested build concurrency
and other compilations so their combined jobs on one machine never exceed 10.
Do not update the local installed Pixels application unless necessary: other
sessions use it.

# Android deployment

The primary Android development device is the USB phone `e2b3b128`.
After building a new APK, update the phone using `adb -s e2b3b128 install -r`.
Always preserve the installed application's data and identity. Never uninstall,
clear application data, or replace a signing key to resolve an update failure.
Keep `.build/android/device.toml` configured with this serial so the Android
build helper installs updated APKs automatically. Do not update unrelated apps.

# Configuration format

All application configuration formats owned by this project must use TOML on
Windows, macOS and Linux, including installer defaults, Desktop preferences and
MCP settings. Preserve user configuration on upgrades and migrate legacy JSON/env
configuration without executing it. Third-party configuration contracts and
runtime data (credentials, databases, protocol messages, release manifests) keep
their required formats.
