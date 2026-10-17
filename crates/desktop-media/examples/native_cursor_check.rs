//! Opt-in live capture/cursor lifecycle check. No keyboard or pointer input.
use pab_desktop_media::NativeBackend;
use pab_protocol::*;
use pab_remote_desktop::DesktopBackend;
use std::{path::PathBuf, time::Duration};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let executable = PathBuf::from(args.next().ok_or("worker path required")?);
    let backend = NativeBackend::new(executable, args.collect());
    assert!(backend.supports_cursor());
    let screen = backend
        .screens()
        .await?
        .into_iter()
        .find(|s| s.primary)
        .ok_or("no primary screen")?;
    let mut config = DesktopVideoConfig {
        session_id: RequestId::new(),
        generation: 1,
        screen,
        fps: 15,
        codec: DesktopCodec::H264AnnexB,
        color: DesktopColor::Bt709Limited,
    };
    assert!(backend.cursor(config.clone()).await.is_err());
    for generation in 1..=2 {
        config.generation = generation;
        let video =
            tokio::time::timeout(Duration::from_secs(20), backend.start(config.clone())).await??;
        let mut stale = config.clone();
        stale.screen.x += 1;
        assert!(backend.cursor(stale).await.is_err());
        for reopen in 0..2 {
            let mut cursor = backend.cursor(config.clone()).await?;
            let update =
                tokio::time::timeout(Duration::from_secs(8), cursor.next_update()).await??;
            assert_eq!(update.header.validate(&config, 1, 0)?, update.bytes.len());
            println!(
                "generation={generation} reopen={reopen} visible={} shape={:?} position={:?} video_requests=0",
                update.header.position.is_some(),
                update.header.image,
                update.header.position
            );
        }
        // Retain an idle cursor while ending video. The child must be reaped
        // without requiring a next_update call or dropping the cursor handle.
        let mut cursor = backend.cursor(config.clone()).await?;
        drop(video);
        tokio::time::timeout(Duration::from_secs(5), backend.idle()).await?;
        assert!(cursor.next_update().await.is_err());
        assert!(backend.cursor(config.clone()).await.is_err());
    }
    println!("cursor capture lease and reopen checks passed");
    Ok(())
}
