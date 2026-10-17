#![cfg(windows)]
use pab_desktop_media::{NativeViewer, software::SoftwareEncoder};
use pab_protocol::*;
use pab_remote_desktop::{CursorUpdate, VideoFrame};
use std::{path::PathBuf, time::Duration};

#[test]
#[ignore = "requires D3D11 GPU; generated pixels only, no visible window or desktop capture"]
fn gpu_cursor_color_masks_scaling_clipping_and_hide_match_pixels() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pab-media-worker"))
        .arg("cursor-overlay-check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(report.contains("cursor overlay cases=30 pixels=1966080 max_error="));
    println!("{}", report.trim());
}
#[tokio::test]
#[ignore = "opens an isolated native window with generated video and cursor pixels; no OS input or desktop capture"]
async fn native_cursor_repaints_static_video_without_video_ack_or_extra_fps() {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_pab-media-worker"));
    let config = DesktopVideoConfig {
        session_id: RequestId::new(),
        generation: 1,
        fps: 15,
        codec: DesktopCodec::H264AnnexB,
        color: DesktopColor::Bt709Limited,
        screen: DesktopScreen {
            id: "generated".into(),
            name: "Generated".into(),
            x: -1920,
            y: -100,
            desktop_width: 640,
            desktop_height: 480,
            pixel_width: 640,
            pixel_height: 480,
            rotation: DesktopRotation::Identity,
            scale_milli: 1000,
            primary: true,
        },
    };
    for software in [false, true] {
        let mut viewer = if software {
            NativeViewer::open_software(&path, &[], &config, false).await
        } else {
            NativeViewer::open(&path, &[], &config, false).await
        }
        .unwrap();
        let mut cursor = viewer.cursor_renderer().unwrap();
        assert!(viewer.cursor_renderer().is_err());
        let mut presentations = viewer.presentations();
        let mut encoder = SoftwareEncoder::new(640, 480, 15).unwrap();
        let pixels = [40, 80, 120, 255].repeat(640 * 480);
        let bytes = encoder.encode_bgra(&pixels, 640 * 4).unwrap().to_vec();
        let frame = VideoFrame {
            header: DesktopFrameHeader {
                version: REMOTE_DESKTOP_SCHEMA_VERSION,
                session_id: config.session_id,
                generation: 1,
                frame_id: 1,
                capture_time_us: 0,
                keyframe: true,
                payload_bytes: bytes.len() as u32,
            },
            bytes,
        };
        viewer.submit(&frame).await.unwrap();
        assert_eq!(presentations.next().await.unwrap().frame_id, 1);
        for sequence in 1..=8 {
            let update = CursorUpdate {
                header: DesktopCursorHeader {
                    version: DESKTOP_CURSOR_SCHEMA_VERSION,
                    session_id: config.session_id,
                    generation: 1,
                    sequence,
                    position: (sequence != 8).then_some(DesktopCursorPosition {
                        x: -1800 + sequence as i32 * 15,
                        y: 100,
                    }),
                    image_id: 1,
                    image: (sequence == 1).then_some(DesktopCursorImage {
                        format: DesktopCursorFormat::PremultipliedBgra,
                        width: 16,
                        height: 16,
                        hotspot_x: 8,
                        hotspot_y: 8,
                    }),
                },
                bytes: if sequence == 1 {
                    [0, 0, 200, 200].repeat(16 * 16)
                } else {
                    vec![]
                },
            };
            tokio::time::timeout(Duration::from_secs(5), cursor.submit(&update))
                .await
                .unwrap()
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(35), presentations.next())
                    .await
                    .is_err()
            );
        }
        cursor.clear().await.unwrap();
        // Only one compressed frame was submitted; nine cursor repaints must
        // not create decoder credit, video metadata, or displayed video FPS.
        let summary = viewer.finish().await.unwrap();
        assert!(summary["displayed"].as_u64().unwrap() <= 1);
        assert!(cursor.clear().await.is_err());
        println!(
            "static native cursor software={software}: 1 video frame, 8 cursor updates, clear; displayed={}",
            summary["displayed"]
        );
    }
}

#[tokio::test]
#[ignore = "opens an isolated generated native window; concurrent video and maximum-size cursor pipe traffic"]
async fn video_and_cursor_commands_do_not_interleave_or_stall_each_other() {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_pab-media-worker"));
    let config = DesktopVideoConfig {
        session_id: RequestId::new(),
        generation: 9,
        fps: 15,
        codec: DesktopCodec::H264AnnexB,
        color: DesktopColor::Bt709Limited,
        screen: DesktopScreen {
            id: "generated".into(),
            name: "Generated".into(),
            x: -500,
            y: -300,
            desktop_width: 320,
            desktop_height: 240,
            pixel_width: 640,
            pixel_height: 480,
            rotation: DesktopRotation::Identity,
            scale_milli: 2000,
            primary: true,
        },
    };
    let mut viewer = NativeViewer::open(&path, &[], &config, false)
        .await
        .unwrap();
    let mut cursor = viewer.cursor_renderer().unwrap();
    let mut presentations = viewer.presentations();
    let mut encoder = SoftwareEncoder::new(640, 480, 15).unwrap();
    let mut frames = Vec::new();
    for frame_id in 1..=16 {
        let pixels = [40 + frame_id as u8 * 5, 80, 120, 255].repeat(640 * 480);
        let bytes = encoder.encode_bgra(&pixels, 640 * 4).unwrap().to_vec();
        frames.push(VideoFrame {
            header: DesktopFrameHeader {
                version: REMOTE_DESKTOP_SCHEMA_VERSION,
                session_id: config.session_id,
                generation: 9,
                frame_id,
                capture_time_us: frame_id * 66666,
                keyframe: frame_id == 1,
                payload_bytes: bytes.len() as u32,
            },
            bytes,
        });
    }
    tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(
            async {
                for frame in frames {
                    viewer.submit(&frame).await.unwrap();
                }
            },
            async {
                for sequence in 1..=16 {
                    let update = CursorUpdate {
                        header: DesktopCursorHeader {
                            version: DESKTOP_CURSOR_SCHEMA_VERSION,
                            session_id: config.session_id,
                            generation: 9,
                            sequence,
                            position: Some(DesktopCursorPosition {
                                x: -500 + sequence as i32 * 10,
                                y: -200,
                            }),
                            image_id: 1,
                            image: (sequence == 1).then_some(DesktopCursorImage {
                                format: DesktopCursorFormat::AndXorBgra,
                                width: 512,
                                height: 512,
                                hotspot_x: 256,
                                hotspot_y: 256,
                            }),
                        },
                        bytes: if sequence == 1 {
                            [255, 255, 255, 0, 0, 0, 0, 0].repeat(512 * 512)
                        } else {
                            vec![]
                        },
                    };
                    cursor.submit(&update).await.unwrap();
                }
                cursor.clear().await.unwrap();
            },
            async {
                loop {
                    if presentations.next().await.unwrap().frame_id == 16 {
                        break;
                    }
                }
            },
        );
    })
    .await
    .unwrap();
    let summary = viewer.finish().await.unwrap();
    assert!(summary["displayed"].as_u64().unwrap() <= 16);
    println!(
        "concurrent native pipe: 16 video frames, 16 cursor updates, 2 MiB first image, clear"
    );
}
