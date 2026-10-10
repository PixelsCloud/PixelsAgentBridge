# Login required for remote control

The initiating Desktop or MCP must have a valid signed-in account before discovering,
adding or controlling a remote device. Receiving devices can run unattended without
account login. Device password verification remains required. HTTP registration,
password login and GitHub login stay available before authentication.

An endpoint key identifies a process; it is not an account login. Unbound process
registration remains available to bootstrap account synchronization, but grants no
device discovery, P2P authorization or Relay forwarding rights. Logout, revoked
sessions and disabled accounts must not fall back to guest control. Existing streams
are closed on authorization recheck; operations already executed are not rolled back.

Bandwidth uses one account limit: 5, 10, 20, 30, 40, 50, 60, 70, 80, 90 or 100 Mbps.
The default is 10 Mbps. No guest limit, service default editor or startup bandwidth
configuration is exposed. Multiple connections of an account share its quota per
Relay node; P2P transport does not consume Relay bandwidth.

Verification covers unauthenticated discovery and authorization rejection, valid login,
logout/revocation, anonymous Relay denial even with an old intent, MCP readiness and
Desktop login prompts, bandwidth dropdown choices and API validation. Windows and
macOS share the client implementation; unattended receiving service behavior remains
available for Windows, macOS and headless Linux. Do not reinstall the local Windows
client used by other sessions during verification.

## Implementation

- Desktop shows a register/sign-in action in Add device. Adding and reconnecting
  saved devices both require account state; password and device-code drafts remain
  in the form while the login dialog is open.
- Bridge checks the local login and matching server-confirmed account revision
  before discovery and device operations. Missing synchronization no longer grants
  access. Logout closes cached connections even when the control server is unreachable;
  a rejected login session closes them without attempting guest fallback.
- Server enforces login on device lookup, presence, directory, network discovery and
  device-peer authorization. Anonymous endpoint registration is only process bootstrap.
  Internal `Guest` endpoint/operator identifiers remain stable process identifiers;
  they do not provide anonymous control privileges.
- Executor already reauthorizes each new request stream and checks existing sessions
  every 5 seconds. MCPs observe account storage changes on notifications or the 3-second
  poll. Relay removes unbound/disabled/revoked operator endpoints from its policy and
  rejects anonymous forwarding even with a leftover connection intent. Receiving
  device endpoints remain admitted without an account.
- The account-limit API and database accept only the dropdown presets. New users
  receive 10 Mbps; administrator changes persist across restarts. Service-level default
  editing, guest bandwidth and startup bandwidth overrides are removed.

## Release boundary

Server and Relay policy schema is now version 6; upgrade them together. Remove the
old `default_user_mbps` and `default_guest_mbps` keys from the private server TOML.
Migration 0006 removes those database columns and initializes unset/invalid account
limits to 10 Mbps without resetting devices, passwords, accounts or device codes.
Desktop/MCP must be updated for the new prompt and local login checks; server checks
also reject anonymous requests from older clients. Production rollout is recorded below.

## Verification completed (2026-10-10)

- Incremental Rust checks: Server, Relay and Bridge; Desktop and Web TypeScript/builds.
- Server: 26 library tests; configuration startup, account control plane, anonymous
  bootstrap/login/revocation, account bandwidth database tests; all 21 Web management
  tests including administrator-only bandwidth changes and disabled/revoked sessions.
- Real TLS/WSS + device sessions: anonymous discovery denied, password verification
  retained, signed-in operators can control simultaneously, logout invalidates the old
  connection, signing in again permits a new connection, task attribution remains stable.
- Protocol: 51 tests. Relay: 12 unit tests plus two real TLS/QUIC integration tests.
  With two connections sharing a test account, measured 3.842 Mbps for a 4 Mbps fixture
  limit, 1.916 Mbps after changing it to 2 Mbps, and 0.000 Mbps after logout. Low fixture
  rates exercise the limiter; the production account API only accepts the listed presets.
- Bridge: readiness rejects missing/unconfirmed/revoked login; terminal archive regression
  remains passing. Test account storage is isolated from the user's real account.
- Browser: four Desktop cases (login gate, revoked-login draft retention, rename, GitHub
  cancellation), three Web navigation/bandwidth cases, and two Web translation tests.

Windows was used for these automated tests. macOS and Linux receiving services were not
exercised on physical devices in this round. The shared account/control implementation
has no new OS-specific branches.

## Production rollout: 1.2.74 (2026-10-10)

- Built the Windows installer and Linux Server/Relay image using existing Release caches.
  Only the installer release version increased; internal component versions stayed fixed.
- Backed up the actual application database and private deployment configuration. Upgraded
  Server, Relay and Web together, removed obsolete bandwidth TOML keys and applied migration
  0006. Account count, device count, device identities and account credential digests matched
  before/after. Existing unset account limits became 10 Mbps.
- Verified public service health, image/binary hashes, served Web index, Relay policy
  acknowledgement, anonymous Web API rejection and managed GitHub egress health.
- Installed the same release on three remote Windows devices through native Pixels MCP
  file transfer and detached installation tasks. All installers exited with code 0;
  all three installed binaries matched their build hashes, Executor services were running,
  and device identity keys remained unchanged. Reconnected to every device and successfully
  executed verification commands after both client and backend upgrades.
- The development computer's installed client was not replaced. Private deployment and
  device-specific receipts are stored under the ignored `.build` directory.
