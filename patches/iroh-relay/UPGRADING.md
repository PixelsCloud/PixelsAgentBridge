# iroh-relay patch maintenance

Pixels Agent Bridge pins `iroh-relay` exactly and carries one narrow forwarding-control
patch. The patch exposes both authenticated Endpoint IDs at the forwarding point and
allows the PAB limiter to return `Allow`, `Wait`, or `Drop`. It does not change iroh's
captive-portal or general TLS behavior.

Before changing the iroh version:

1. Run `python scripts/verify_iroh_vendor.py` and keep the existing report with the
   upgrade work.
2. Fetch and copy the exact new crates.io source into `vendor/iroh-relay`.
3. Apply `patches/iroh-relay/1.2.0-forwarding-control.patch` from the new crate root.
   Resolve conflicts by following the new upstream forwarding path; do not copy the old
   files wholesale.
4. Review the resulting diff, rename the patch for the new version, and update every
   upstream/patched SHA-256 entry and the complete vendor tree SHA-256 in
   `manifest.json`.
5. Run the verifier with `--write-patch`, inspect the generated patch, then run it again
   without that flag.
6. Run the full Debug workspace tests and Clippy. Repeat the real TLS Relay tests for
   Endpoint admission, source/destination identity, `Wait` backpressure, Team/member
   aggregation, reconnect sharing, QUIC address discovery, and Windows/Linux clients.

An upgrade is incomplete if only compilation succeeds. The forwarding-path tests and
real TLS transfer results are the compatibility contract.
