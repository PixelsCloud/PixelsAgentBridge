# Pixels network recovery patch

`Endpoint::reprobe_network()` explicitly reruns direct-address discovery and
peer-path validation after an application's authenticated control connection
recovers. The existing `network_change()` API asks the OS interface monitor to
resample, which suppresses notifications when local addresses are unchanged.
A router/NAT interruption can require rediscovery without an interface change.

The added API uses the existing minor-link-change actor path. It preserves
sockets, endpoint identity, authenticated QUIC connections and streams. The
Executor invokes it once per recovered control generation, including when watch
updates coalesce intermediate disconnected states. Ordinary status notifications
and initial authentication do not trigger repeated probing.

Regression coverage: `pab-transport`'s
`network_reprobe_preserves_an_existing_direct_stream`, plus Executor's
`network_recovery_*` tests. This patch does not change iroh's license.
