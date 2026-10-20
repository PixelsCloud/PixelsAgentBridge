//! Opt-in 90 portrait fixture check. No keyboard or pointer input.
use pab_desktop_media::NativeBackend;
use pab_protocol::*;
use pab_remote_desktop::DesktopBackend;
use std::{path::PathBuf, time::Duration};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let executable = PathBuf::from(args.next().ok_or("worker path required")?);
    let backend = NativeBackend::new(executable, args.collect());
    let selected: Vec<_> = backend
        .screens()
        .await?
        .into_iter()
        .filter(|s| {
            s.x == -1080
                && s.y == 0
                && s.desktop_width == 1080
                && s.desktop_height == 1920
                && s.rotation == DesktopRotation::Clockwise90
        })
        .collect();
    assert_eq!(selected.len(), 1, "requires the owned portrait fixture");
    let mut config = DesktopVideoConfig {
        session_id: RequestId::new(),
        generation: 1,
        screen: selected[0].clone(),
        fps: 15,
        codec: DesktopCodec::H264AnnexB,
        color: DesktopColor::Bt709Limited,
    };
    for generation in 1..=2 {
        config.generation = generation;
        let mut video =
            tokio::time::timeout(Duration::from_secs(10), backend.start(config.clone())).await??;
        let mut cursor = backend.cursor(config.clone()).await?;
        // Let the real DXGI cursor worker encounter the failing portrait output
        // while video has zero credits. Its fallback request must be serviced.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let update = tokio::time::timeout(Duration::from_secs(8), cursor.next_update()).await??;
        assert_eq!(update.header.validate(&config, 1, 0)?, update.bytes.len());
        println!(
            "generation={generation} cursor_ready=true video_requests=0 visible={}",
            update.header.position.is_some()
        );
        let frame = tokio::time::timeout(Duration::from_secs(8), video.next_frame()).await??;
        assert!(frame.keyframe && !frame.bytes.is_empty());
        drop(cursor);
        let mut cursor = backend.cursor(config.clone()).await?;
        let update = tokio::time::timeout(Duration::from_secs(8), cursor.next_update()).await??;
        assert_eq!(update.header.validate(&config, 1, 0)?, update.bytes.len());
        drop(video);
        tokio::time::timeout(Duration::from_secs(5), backend.idle()).await?;
        assert!(cursor.next_update().await.is_err());
        assert!(backend.cursor(config.clone()).await.is_err());
        println!("generation={generation} encoded_first_frame=true cursor_reopened=true workers_reaped=true");
    }
    println!("portrait cursor coordination passed");
    Ok(())
}
