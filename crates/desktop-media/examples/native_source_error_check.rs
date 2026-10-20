//! Opt-in Windows startup error check. Enumerates screens, then requests an
//! invented screen ID; it captures no desktop pixels and sends no OS input.
use pab_desktop_media::NativeBackend;
use pab_protocol::*;
use pab_remote_desktop::DesktopBackend;
use std::{path::PathBuf, time::Duration};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let executable = PathBuf::from(args.next().ok_or("worker path required")?);
    let backend = NativeBackend::new(executable, args.collect());
    let mut screen = backend
        .screens()
        .await?
        .into_iter()
        .next()
        .ok_or("no screens")?;
    screen.id = format!("missing-{}", RequestId::new());
    let config = DesktopVideoConfig {
        session_id: RequestId::new(),
        generation: 1,
        screen,
        fps: 15,
        codec: DesktopCodec::H264AnnexB,
        color: DesktopColor::Bt709Limited,
    };
    let result = tokio::time::timeout(Duration::from_secs(10), backend.start(config)).await?;
    let error = match result {
        Ok(_) => return Err("Invented screen was accepted".into()),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(message.contains("Selected screen disappeared"), "{message}");
    tokio::time::timeout(Duration::from_secs(5), backend.idle()).await?;
    println!(
        "{}",
        serde_json::json!({"native_error_preserved":true,"message":message,"worker_reaped":true})
    );
    Ok(())
}
