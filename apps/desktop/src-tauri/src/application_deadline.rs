use std::time::Duration;

/// Only for the dedicated application helper: a native Shell call cannot be
/// cancelled by dropping its blocking task. Retire this process, not its apps.
pub(super) async fn run<T: Send + 'static>(
    id: pab_protocol::RequestId,
    operation: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    match tokio::time::timeout(
        Duration::from_secs(20),
        tokio::task::spawn_blocking(operation),
    )
    .await
    {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => {
            tracing::error!(%id, "application call exceeded deadline; retiring helper without replay");
            std::process::exit(1);
        }
    }
}
