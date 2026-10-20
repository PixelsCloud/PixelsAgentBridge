pub fn validate_url(value: &str, scheme: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|_| "Invalid server URL")?;
    if url.scheme() != scheme
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || value.chars().any(char::is_whitespace)
        || value.contains(',')
    {
        return Err(
            "Use wss:// control and https:// relay URLs without credentials or whitespace".into(),
        );
    }
    Ok(())
}

pub fn validate_pair(control: &str, relays: &[String]) -> Result<(), String> {
    if control.is_empty() && relays.is_empty() {
        return Ok(());
    }
    if control.is_empty() || relays.is_empty() {
        return Err("Fill in both control and relay URLs, or leave both empty".into());
    }
    validate_url(control, "wss")?;
    for relay in relays {
        validate_url(relay, "https")?;
    }
    Ok(())
}
