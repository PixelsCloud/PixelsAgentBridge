# Development

Use the pinned Rust toolchain and checked-in Cargo/npm lockfiles. Build prerequisites,
platform packaging and version allocation are described in [BUILDING.md](BUILDING.md).
Keep the combined Cargo, CMake, Gradle and nested build concurrency at or below
10 jobs per machine. Ordinary checks do not allocate release versions.

## Repository layout

- `crates/`: shared protocol, transport, client, executor, server and Relay code.
- `apps/`: Desktop, Web, Android and agent integrations.
- `packaging/` and `scripts/`: reproducible builds, deployment templates and checks.
- `vendor/` and `patches/`: pinned third-party code, local patches and licenses.
- `acceptance/fixtures/`: deterministic test inputs; `acceptance/tool-coverage.json`
  is the MCP test contract. Keep these inputs in Git.
- `experiments/remote-desktop/reliable_media.rs` and `media_child.rs`: source inputs
  of the registered `reliable-media` Cargo example; keep both in Git.

## Checks

Run from the repository root, selecting the packages affected by a change:

```powershell
cargo fmt --all -- --check
python scripts/verify_iroh_vendor.py
cargo test -p <package> -j 8
cargo clippy -p <package> --all-targets -j 8 -- -D warnings
```

Frontend commands live in each app's `package.json`. Install dependencies with
`npm ci` from that app before running its tests. For Desktop page work, run
`npm run dev:desktop` in `apps/desktop`; restart after Rust changes. A browser
opening the Vite URL does not provide Tauri's native APIs.

Integration tests must use isolated databases, test accounts and temporary
directories. Never point test configuration at a production database. Hardware
and installed-package checks require their actual target platform; a source
check is not a substitute for those checks.

## Files that stay local

Keep task plans, progress histories, one-off experiments, screenshots, run reports,
machine instructions (`AGENTS.md`) and private credentials on the development
machine. The ignore rules retain these files locally while excluding them from
future commits. Use `.build/` for generated reports and build artifacts;
`acceptance/` for local run evidence, apart from the tracked fixtures and contract;
`design/` for mockups, apart from reusable `design/brand/` assets.

Commit source, reusable automated tests, lockfiles, configuration examples,
licenses, build scripts and maintained usage/deployment documentation. Do not
force-add ignored run evidence. Removing a tracked file with `git rm --cached`
keeps its local contents; it does not erase that file from earlier commits.

Project-owned application settings use TOML. Secrets and live deployment settings
belong in ignored local storage, not example files. Preserve user configuration,
credentials and device identities during upgrades. See [configuration](docs/CONFIGURATION.md),
[deployment](WEB_DEPLOYMENT.md) and [release instructions](docs/RELEASE.md).
