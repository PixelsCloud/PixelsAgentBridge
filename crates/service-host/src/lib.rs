//! Native service lifecycle shared by the CENTRAL Server and Relay, never Executor.
use std::ffi::OsString;

pub fn arguments(
    args: impl IntoIterator<Item = OsString>,
    default_name: &str,
) -> Result<(Option<String>, Vec<OsString>), String> {
    let mut service = false;
    let mut name = None;
    let mut rest = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--service" {
            if service {
                return Err("--service must appear once".into());
            }
            service = true;
        } else if arg == "--service-name" {
            if name.is_some() {
                return Err("--service-name must appear once".into());
            }
            name = Some(
                args.next()
                    .and_then(|v| v.into_string().ok())
                    .ok_or("--service-name requires a name")?,
            );
        } else {
            rest.push(arg);
        }
    }
    if name.is_some() && !service {
        return Err("--service-name requires --service".into());
    }
    if !service {
        return Ok((None, rest));
    }
    let name = name.unwrap_or_else(|| default_name.into());
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(
            "service name requires 1-80 ASCII letters, digits, dots, underscores or hyphens".into(),
        );
    }
    if name.eq_ignore_ascii_case("PixelsAgentBridgeExecutor") {
        return Err("the Executor service name is reserved".into());
    }
    Ok((Some(name), rest))
}

pub async fn stop_requested(stop: Option<tokio::sync::watch::Receiver<bool>>) {
    if let Some(mut stop) = stop {
        while !*stop.borrow_and_update() {
            if stop.changed().await.is_err() {
                break;
            }
        }
    } else {
        #[cfg(unix)]
        {
            if let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
            } else {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{ServiceFuture, run};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_flags_preserve_config_paths_and_never_target_executor() {
        let args = [
            "serve",
            "--config",
            "C:\\Program Data\\Pixels Server\\server.toml",
            "--service",
            "--service-name",
            "PixelsRelayTest",
        ];
        let (name, rest) = arguments(args.into_iter().map(OsString::from), "PixelsServer").unwrap();
        assert_eq!(name.as_deref(), Some("PixelsRelayTest"));
        assert_eq!(rest.len(), 3);
        assert!(
            arguments(
                ["--service", "--service-name", "PixelsAgentBridgeExecutor"]
                    .into_iter()
                    .map(OsString::from),
                "x"
            )
            .is_err()
        );
        assert!(arguments(["--service-name", "x"].into_iter().map(OsString::from), "x").is_err());
    }
}
