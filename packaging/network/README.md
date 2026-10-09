# GitHub outbound connectivity

The account backend needs HTTPS access to `github.com:443` and `api.github.com:443`.
Use normal DNS and verified TLS. Do not add static GitHub addresses to `/etc/hosts`,
Docker `extra_hosts`, or application code.

Set the optional `PAB_GITHUB_PROXY_URL` only for the GitHub HTTP client. An existing
managed HTTP/HTTPS CONNECT or SOCKS5H proxy is suitable. With SOCKS5H, the egress
host resolves the destination; the backend still verifies GitHub's TLS certificate.
No TLS interception, custom CA or forwarding of the GitHub secret to a proxy API
is needed.

If reusing a privately administered SSH host, OpenSSH `-D` provides the SOCKS5
transport without a custom proxy implementation. The accompanying examples need
deployment-specific addresses and a dedicated key; never commit those values.

1. Create a dedicated service user on both hosts. Keep the private SSH key on the
   backend host and pin the egress SSH host key through a trusted administrative
   connection. Do not disable `StrictHostKeyChecking`.
2. Restrict the egress user's authorized key with `restrict,port-forwarding`,
   `command="/usr/sbin/nologin"`, `permitopen="github.com:443"` and
   `permitopen="api.github.com:443"`. A scoped `sshd_config` Match User should allow
   local TCP forwarding only and deny remote listeners, passwords, PTYs, agent
   forwarding and X11. Validate with `sshd -t` before reloading.
3. Adapt `github-egress.ssh-config.example`, binding the SOCKS listener only to a
   private interface reachable by Backend. Allow its port in the host firewall
   only from the application's Docker bridge/subnet, never from the Internet.
   Revalidate the address/rule if recreating the Docker network.
4. Install the service example and enable it with systemd. Set
   `PAB_GITHUB_PROXY_URL=socks5h://<private-interface>:39081` in the private Compose
   environment, then recreate only Backend. No GitHub IP mappings are required.
5. From Backend, check HTTPS access to both domains with `curl --proxy` and TLS
   verification enabled. Test that other destinations are rejected. Kill only the
   egress service's main process and verify systemd restarts it and HTTPS recovers.
6. Monitor service state and periodically probe both domains over this same path.
   The health-check/timer examples exit nonzero and write a journal error on
   failure; connect failed systemd units to your existing alerting. They do not
   use OAuth codes or credentials. Keep application health separate: a GitHub
   outage must not restart the device-control backend.

Use `journalctl -u pab-github-egress` for tunnel failures and Backend logs for the
OAuth stage/error class. A failed or ambiguous code exchange requires a fresh
login attempt; never resend a potentially consumed authorization code. Before
removing the egress, test direct access from Backend, unset the proxy, recreate
Backend, then disable the egress service/timer and remove its scoped firewall rule.

References: [OpenSSH dynamic forwarding](https://man.openbsd.org/ssh#D),
[reqwest proxy support](https://docs.rs/reqwest/0.12.28/reqwest/struct.Proxy.html).
