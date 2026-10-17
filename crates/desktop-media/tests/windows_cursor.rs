#![cfg(windows)]
use pab_desktop_media::NativeBackend;
use pab_protocol::*;
use pab_remote_desktop::DesktopBackend;
use std::{path::PathBuf, time::Duration};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Point {
    x: i32,
    y: i32,
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetPhysicalCursorPos(point: *mut Point) -> i32;
    fn SetThreadDpiAwarenessContext(context: isize) -> isize;
}
fn physical_position() -> Point {
    let mut point = Point { x: 0, y: 0 };
    // Use the same per-monitor coordinate context as native capture; restore
    // immediately, before any await or assertion.
    // SAFETY: -4 is DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2. The returned
    // context belongs to this thread; Windows writes one valid stack POINT.
    let previous = unsafe { SetThreadDpiAwarenessContext(-4) };
    assert_ne!(previous, 0);
    let success = unsafe { GetPhysicalCursorPos(&mut point) };
    unsafe { SetThreadDpiAwarenessContext(previous) };
    assert_ne!(success, 0);
    point
}

#[test]
fn generated_dxgi_shapes_preserve_masks_alpha_padding_and_hotspots() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pab-media-worker"))
        .arg("cursor-shape-check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "cursor shape checks=52"
    );
}

#[tokio::test]
#[ignore = "requires Windows interactive GPU desktop; only reads cursor metadata, no OS input"]
async fn native_cursor_is_bound_to_video_and_reaps_while_retained() {
    let backend = NativeBackend::new(
        PathBuf::from(env!("CARGO_BIN_EXE_pab-media-worker")),
        vec![],
    );
    let screens = backend.screens().await.unwrap();
    let screen = screens.into_iter().find(|s| s.primary).unwrap();
    let mut config = DesktopVideoConfig {
        session_id: RequestId::new(),
        generation: 1,
        screen,
        fps: 15,
        codec: DesktopCodec::H264AnnexB,
        color: DesktopColor::Bt709Limited,
    };
    assert!(backend.supports_cursor());
    assert!(backend.cursor(config.clone()).await.is_err());
    for generation in 1..=2 {
        config.generation = generation;
        // Start the real video process, but do not ask it for any frame. This
        // verifies the cursor has no dependency on video changes or credit.
        let video = tokio::time::timeout(Duration::from_secs(10), backend.start(config.clone()))
            .await
            .unwrap()
            .unwrap();
        let mut stale = config.clone();
        stale.screen.x += 1;
        assert!(backend.cursor(stale).await.is_err());
        let mut cursor = backend.cursor(config.clone()).await.unwrap();
        let before = physical_position();
        let update = tokio::time::timeout(Duration::from_secs(8), cursor.next_update())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(update.header.session_id, config.session_id);
        assert_eq!(update.header.generation, generation);
        assert_eq!(update.header.sequence, 1);
        assert_eq!(
            update.header.validate(&config, 1, 0).unwrap(),
            update.bytes.len()
        );
        let after = physical_position();
        let checked_position = before == after && update.header.position.is_some();
        if checked_position {
            assert_eq!(
                update.header.position,
                Some(DesktopCursorPosition {
                    x: after.x,
                    y: after.y
                })
            );
        }
        println!(
            "native cursor generation={generation} visible={} image_bytes={} video_frames_requested=0 physical_position_checked={checked_position}",
            update.header.position.is_some(),
            update.bytes.len()
        );
        drop(cursor);
        // Ending only the cursor must preserve the active video generation.
        let mut cursor = backend.cursor(config.clone()).await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(8), cursor.next_update())
                .await
                .unwrap()
                .unwrap()
                .header
                .sequence,
            1
        );
        drop(video);
        // Keep the cursor handle alive: capture retirement must still reap it.
        tokio::time::timeout(Duration::from_secs(5), backend.idle())
            .await
            .unwrap();
        assert!(cursor.next_update().await.is_err());
        assert!(backend.cursor(config.clone()).await.is_err());
        drop(cursor);
    }
}
