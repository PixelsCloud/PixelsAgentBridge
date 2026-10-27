//! HTTPS defaults and bounded, credential-free transport diagnostics for native clients.
use std::{error::Error, fmt, time::Duration};

pub const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(30);
pub const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(20);
pub const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(900);

pub fn builder(timeout: Duration) -> Result<reqwest::ClientBuilder, Failure> {
    // Both web PKI and native roots are enabled by Cargo features. Validation of
    // certificate chains and hostnames remains enabled, including through proxies.
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    let https = environment("HTTPS_PROXY", "https_proxy");
    let all = environment("ALL_PROXY", "all_proxy");
    // hyper-util fills protocol-specific system proxies before falling back to
    // ALL_PROXY. Honor an explicitly inherited HTTPS/ALL proxy first, including
    // SOCKS5h's remote DNS, while preserving NO_PROXY. No environment override
    // means reqwest retains its normal OS HTTP/HTTPS proxy discovery.
    if https.is_some() || all.is_some() {
        if let Some(value) = https {
            let proxy =
                reqwest::Proxy::https(value).map_err(|error| failure("proxy.configure", &error))?;
            builder = builder.proxy(proxy.no_proxy(reqwest::NoProxy::from_env()));
        }
        if let Some(value) = environment("HTTP_PROXY", "http_proxy") {
            let proxy =
                reqwest::Proxy::http(value).map_err(|error| failure("proxy.configure", &error))?;
            builder = builder.proxy(proxy.no_proxy(reqwest::NoProxy::from_env()));
        }
        if let Some(value) = all {
            let proxy =
                reqwest::Proxy::all(value).map_err(|error| failure("proxy.configure", &error))?;
            builder = builder.proxy(proxy.no_proxy(reqwest::NoProxy::from_env()));
        }
    }
    Ok(builder)
}

fn environment(upper: &str, lower: &str) -> Option<String> {
    std::env::var(upper)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var(lower).ok().filter(|value| !value.is_empty()))
}

pub fn client(timeout: Duration, extra_ca_pem: Option<&[u8]>) -> Result<reqwest::Client, Failure> {
    with_extra_ca(builder(timeout)?, extra_ca_pem)?
        .build()
        .map_err(|error| failure("http.initialize", &error))
}

fn with_extra_ca(
    mut builder: reqwest::ClientBuilder,
    extra_ca_pem: Option<&[u8]>,
) -> Result<reqwest::ClientBuilder, Failure> {
    if let Some(pem) = extra_ca_pem {
        let certificates =
            reqwest::Certificate::from_pem_bundle(pem).map_err(|_| Failure::invalid_ca())?;
        if certificates.is_empty() {
            return Err(Failure::invalid_ca());
        }
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Timeout,
    Dns,
    Tls,
    Proxy,
    ProxyAuthentication,
    Connection,
    Configuration,
    InvalidCa,
    Response,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub detail: &'static str,
}

impl Failure {
    pub(crate) fn invalid_ca() -> Self {
        tracing::warn!(
            detail = "invalid_or_empty_ca_bundle",
            "HTTPS CA configuration rejected"
        );
        Self {
            kind: FailureKind::InvalidCa,
            detail: "invalid_or_empty_ca_bundle",
        }
    }

    pub(crate) fn unreadable_ca(error: &std::io::Error) -> Self {
        tracing::warn!(io_kind = ?error.kind(), detail = "configured_ca_unreadable", "HTTPS CA file could not be read");
        Self {
            kind: FailureKind::InvalidCa,
            detail: "configured_ca_unreadable",
        }
    }

    pub fn account_code(self) -> &'static str {
        match self.kind {
            FailureKind::Timeout => "account_network_timeout",
            FailureKind::Dns => "account_dns_failed",
            FailureKind::Tls => "account_tls_failed",
            FailureKind::Proxy => "account_proxy_failed",
            FailureKind::ProxyAuthentication => "account_proxy_authentication_required",
            FailureKind::InvalidCa => "account_ca_invalid",
            FailureKind::Configuration => "account_network_configuration_invalid",
            FailureKind::Connection | FailureKind::Response => "account_connection_failed",
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            FailureKind::Timeout => "The HTTPS request timed out. Check the network and proxy",
            FailureKind::Dns => "Could not resolve the server or proxy hostname",
            FailureKind::Tls => "TLS verification or handshake failed. Check system time, trusted certificates and HTTPS proxy settings",
            FailureKind::Proxy => "Could not connect through the proxy. Check its address, port and protocol",
            FailureKind::ProxyAuthentication => "The proxy requires authentication",
            FailureKind::Connection => "Could not establish the HTTPS connection. Check the network and proxy",
            FailureKind::Configuration => "Could not prepare the HTTPS connection. Check proxy and TLS configuration",
            FailureKind::InvalidCa => "The configured CA certificate file is unreadable, invalid or empty",
            FailureKind::Response => "The HTTPS response was interrupted or could not be decoded",
        })
    }
}

impl Error for Failure {}

/// Inspect underlying causes only to derive fixed labels. Never return/log the
/// raw error: it can contain proxy credentials, URLs, OAuth codes or response data.
pub fn failure(operation: &'static str, error: &reqwest::Error) -> Failure {
    let mut messages = String::new();
    let mut tls = false;
    let mut io_kind = None;
    // The outer reqwest message contains the request URL. Do not classify from
    // it either: words in a secret query string must not influence the category.
    let mut source: Option<&(dyn Error + 'static)> = error.source();
    for _ in 0..16 {
        let Some(current) = source else { break };
        messages.extend(
            current
                .to_string()
                .chars()
                .take(2048)
                .flat_map(char::to_lowercase),
        );
        messages.push('\n');
        tls |= current.downcast_ref::<rustls::Error>().is_some();
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            io_kind = Some(io.kind());
        }
        source = current.source();
    }
    let has = |values: &[&str]| values.iter().any(|value| messages.contains(value));
    let (kind, detail) = if error.is_timeout() {
        (FailureKind::Timeout, "request_timeout")
    } else if error.status().is_some_and(|status| status.as_u16() == 407)
        || has(&["proxy authentication", "tunnel", "proxy", "socks"])
            && has(&[
                "407",
                "authentication required",
                "authorization required",
                "no acceptable auth",
            ])
    {
        (
            FailureKind::ProxyAuthentication,
            "proxy_authentication_required",
        )
    } else if has(&[
        "dns error",
        "dns resolution",
        "failed to lookup address",
        "name or service not known",
        "nodename nor servname",
        "getaddrinfo",
    ]) {
        (FailureKind::Dns, "hostname_resolution_failed")
    } else if tls
        || has(&[
            "certificate",
            "tls handshake",
            "invalid peer",
            "unknownissuer",
            "unknown issuer",
        ])
    {
        let detail = if has(&["unknownissuer", "unknown issuer"]) {
            "certificate_unknown_issuer"
        } else if has(&[
            "notvalidforname",
            "not valid for name",
            "not valid for the given name",
            "hostname mismatch",
        ]) {
            "certificate_hostname_mismatch"
        } else if has(&["expired"]) {
            "certificate_expired"
        } else if has(&["notvalidyet", "not valid yet"]) {
            "certificate_not_yet_valid"
        } else {
            "tls_handshake_failed"
        };
        (FailureKind::Tls, detail)
    } else if has(&["proxy", "tunnel", "socks"]) {
        (FailureKind::Proxy, "proxy_connection_failed")
    } else if error.is_connect() {
        (
            FailureKind::Connection,
            match io_kind {
                Some(std::io::ErrorKind::ConnectionRefused) => "connection_refused",
                Some(std::io::ErrorKind::NetworkUnreachable) => "network_unreachable",
                _ => "connection_failed",
            },
        )
    } else if error.is_builder() {
        (FailureKind::Configuration, "client_configuration_failed")
    } else {
        (FailureKind::Response, "response_interrupted_or_invalid")
    };
    let result = Failure { kind, detail };
    let origin = error.url().map(|url| url.origin().ascii_serialization());
    tracing::warn!(
        operation, origin = origin.as_deref().unwrap_or("unavailable"),
        kind = ?kind, detail,
        https_proxy_environment = proxy_environment("HTTPS_PROXY", "https_proxy"),
        all_proxy_environment = proxy_environment("ALL_PROXY", "all_proxy"),
        no_proxy_environment = environment("NO_PROXY", "no_proxy").is_some(),
        system_proxy_mode = system_proxy_mode(),
        "HTTPS request failed"
    );
    result
}

fn proxy_environment(upper: &str, lower: &str) -> &'static str {
    let Some(value) = environment(upper, lower) else {
        return "unset";
    };
    // A whitelist records only the protocol, never a username/password/host/URL.
    match reqwest::Url::parse(&value)
        .ok()
        .as_ref()
        .map(|url| url.scheme())
    {
        Some("http") => "http",
        Some("https") => "https",
        Some("socks5") => "socks5",
        Some("socks5h") => "socks5h",
        Some("socks4") => "socks4",
        Some("socks4a") => "socks4a",
        _ => "configured",
    }
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
use macos::system_proxy_mode;

#[cfg(not(target_os = "macos"))]
fn system_proxy_mode() -> &'static str {
    "platform_default"
}

#[cfg(test)]
mod tests;
