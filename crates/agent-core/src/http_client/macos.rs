// Read only the enabled flags. Proxy URLs, credentials and PAC contents stay private.
pub(super) fn system_proxy_mode() -> &'static str {
    use system_configuration::{
        core_foundation::{number::CFNumber, string::CFString},
        dynamic_store::SCDynamicStoreBuilder,
    };
    let Some(store) = SCDynamicStoreBuilder::new("PixelsAgentBridge.NetworkDiagnostics").build()
    else {
        return "unavailable";
    };
    let Some(settings) = store.get_proxies() else {
        return "unavailable";
    };
    let enabled = |key: &str| {
        settings
            .find(&CFString::new(key))
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|value| value.to_i32())
            == Some(1)
    };
    match (
        enabled("HTTPSEnable"),
        enabled("HTTPEnable"),
        enabled("ProxyAutoConfigEnable"),
        enabled("ProxyAutoDiscoveryEnable"),
    ) {
        (true, _, _, _) => "https",
        (false, true, _, _) => "http_only",
        (false, false, true, _) => "pac_only",
        (false, false, false, true) => "auto_discovery_only",
        _ => "none",
    }
}
