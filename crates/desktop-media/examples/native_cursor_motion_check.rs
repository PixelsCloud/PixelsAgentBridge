//! Opt-in live source check: moves the pointer within the selected screen and
//! restores its starting position. No clicks or key events. Use an idle desktop.
use pab_desktop_media::NativeBackend;
use pab_protocol::*;
use pab_remote_desktop::DesktopBackend;
use std::{path::PathBuf, time::Duration};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let executable = PathBuf::from(args.next().ok_or("worker required")?);
    let backend = NativeBackend::new(executable, args.collect()).with_input();
    let result = run(&backend).await;
    // Keep the runtime alive for worker reaping even if input startup fails
    // before the motion/restore section (for example, no XDG_RUNTIME_DIR).
    tokio::time::timeout(Duration::from_secs(5), backend.idle()).await?;
    result
}

async fn run(backend: &NativeBackend) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    assert!(backend.supports_cursor() && backend.supports_input());
    let screen = backend
        .screens()
        .await?
        .into_iter()
        .find(|s| s.primary)
        .ok_or("primary screen required")?;
    let config = DesktopVideoConfig {
        session_id: RequestId::new(),
        generation: 1,
        screen,
        fps: 15,
        codec: DesktopCodec::H264AnnexB,
        color: DesktopColor::Bt709Limited,
    };
    let video = backend.start(config.clone()).await?;
    let mut cursor = backend.cursor(config.clone()).await?;
    let first = tokio::time::timeout(Duration::from_secs(5), cursor.next_update()).await??;
    let original = first
        .header
        .position
        .ok_or("visible starting pointer required")?;
    let normalize = |x: i32, y: i32| -> (u16, u16) {
        let s = &config.screen;
        (
            ((i64::from(x) - i64::from(s.x)).clamp(0, i64::from(s.desktop_width) - 1) * 65535
                / (i64::from(s.desktop_width) - 1)) as u16,
            ((i64::from(y) - i64::from(s.y)).clamp(0, i64::from(s.desktop_height) - 1) * 65535
                / (i64::from(s.desktop_height) - 1)) as u16,
        )
    };
    let mut input = backend.input(config.clone()).await?;
    let result:Result<(),Box<dyn std::error::Error + Send + Sync>> = async {
        let mut expected=(0,0);
        let direction = if i64::from(original.x)+40 < i64::from(config.screen.x)+i64::from(config.screen.desktop_width) { 1 } else { -1 };
        for delta in [10,20,30] {
            let (x,y)=normalize(original.x.saturating_add(delta*direction),original.y);
            input.apply(RemoteDesktopInput::Move { x,y }).await?;
            expected=config.screen.input_position(x,y)?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // No cursor read during the burst and no video request at all. The
        // next update must contain the newest position, not replay the burst.
        let update=tokio::time::timeout(Duration::from_secs(5),cursor.next_update()).await??;
        let position=update.header.position.ok_or("pointer unexpectedly hidden")?;
        assert!((position.x-expected.0).abs()<=1 && (position.y-expected.1).abs()<=1);
        assert_eq!(update.header.sequence,2);
        println!("motion_without_video_requests=true newest_position={position:?} expected={expected:?} shape_bytes={}",update.bytes.len());
        Ok(())
    }.await;
    let (x, y) = normalize(original.x, original.y);
    input.apply(RemoteDesktopInput::Move { x, y }).await?;
    let restored = tokio::time::timeout(Duration::from_secs(5), cursor.next_update()).await;
    drop(input);
    drop(cursor);
    drop(video);
    tokio::time::timeout(Duration::from_secs(5), backend.idle()).await?;
    result?;
    let restored = restored??
        .header
        .position
        .ok_or("restored pointer hidden")?;
    assert!((restored.x - original.x).abs() <= 1 && (restored.y - original.y).abs() <= 1);
    println!("restored_position={restored:?} original_position={original:?}");
    println!("cursor motion check passed; starting pointer restored; video_requests=0");
    Ok(())
}
