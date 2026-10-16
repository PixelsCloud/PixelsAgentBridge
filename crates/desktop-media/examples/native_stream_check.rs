//! Opt-in GPU acceptance of the production streaming adapter. No desktop pixels
//! or compressed video are saved. Synthetic mode checks four pixels per frame.
use pab_desktop_media::{NativeBackend, NativeViewer};
use pab_protocol::*;
use pab_remote_desktop::{DesktopBackend, VideoFrame};
use std::{
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
type Error = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(45), run()).await?
}

async fn run() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 || !matches!(args[1].as_str(), "synthetic" | "animate") {
        return Err("usage: native_stream_check WORKER synthetic|animate 15|30".into());
    }
    let executable = PathBuf::from(&args[0]);
    let fps: u16 = args[2].parse()?;
    if !matches!(fps, 15 | 30) {
        return Err("FPS must be 15 or 30".into());
    }
    let screen = NativeBackend::new(executable.clone(), vec![])
        .screens()
        .await?
        .into_iter()
        .find(|s| s.primary)
        .ok_or("no primary screen")?;
    let config = DesktopVideoConfig {
        session_id: RequestId::new(),
        generation: 1,
        screen,
        codec: DesktopCodec::H264AnnexB,
        color: DesktopColor::Bt709Limited,
        fps,
    };
    let mut command = Command::new(&executable);
    command
        .args(["capture", &config.screen.id, &fps.to_string(), &args[1]])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut source = command.spawn()?;
    let mut input = source.stdin.take().unwrap();
    let mut packets = source.stdout.take().unwrap();
    if packets.read_u32_le().await? != 0x31424150
        || packets.read_u32_le().await? != config.screen.pixel_width
        || packets.read_u32_le().await? != config.screen.pixel_height
    {
        return Err("source metadata mismatch".into());
    }
    let verify = args[1] == "synthetic";
    let mut viewer = NativeViewer::open(&executable, &[], &config, verify).await?;
    let mut presentations = viewer.presentations();
    let warmup = u64::from(fps);
    let total = warmup * 6;
    let receive_output = async {
        loop {
            let header = presentations.next().await?;
            if header.frame_id == total {
                return Ok::<(), Error>(());
            }
        }
    };
    let submit = async {
        let mut start = Instant::now();
        for index in 0..total {
            input.write_u8(1).await?;
            let size = packets.read_u32_le().await?;
            if size == 0 || size as usize > DESKTOP_MAX_FRAME_BYTES {
                return Err("invalid packet size".into());
            }
            if packets.read_u64_le().await? != index {
                return Err("source sequence mismatch".into());
            }
            let capture_time_us = packets.read_u64_le().await?;
            let key = packets.read_u32_le().await?;
            if key > 1 {
                return Err("invalid keyframe flag".into());
            }
            let mut bytes = vec![0; size as usize];
            packets.read_exact(&mut bytes).await?;
            viewer
                .submit(&VideoFrame {
                    header: DesktopFrameHeader {
                        version: REMOTE_DESKTOP_SCHEMA_VERSION,
                        session_id: config.session_id,
                        generation: config.generation,
                        frame_id: index + 1,
                        capture_time_us,
                        keyframe: key == 1,
                        payload_bytes: size,
                    },
                    bytes,
                })
                .await?;
            if index + 1 == warmup {
                start = Instant::now();
            }
        }
        let display = viewer.finish().await?;
        Ok::<_, Error>((display, start.elapsed()))
    };
    let ((display, elapsed), ()) = tokio::try_join!(submit, receive_output)?;
    input.write_u8(0).await?;
    if !source.wait().await?.success() {
        return Err("source failed on shutdown".into());
    }
    let confirmed = display["displayed"]
        .as_u64()
        .ok_or("missing display count")?;
    let effective_fps = confirmed.saturating_sub(warmup) as f64 / elapsed.as_secs_f64();
    let passed = confirmed <= total
        && effective_fps > 12.0
        && effective_fps >= f64::from(fps) * 0.9
        && (!verify
            || (display["test_pixel_checks"] == total * 4
                && display["test_max_rgb_error"]
                    .as_u64()
                    .is_some_and(|n| n <= 12)));
    println!(
        "{}",
        serde_json::json!({"passed":passed,"source":args[1],"target_fps":fps,
        "dimensions":[config.screen.pixel_width,config.screen.pixel_height],"submitted":total,"warmup":warmup,"display":display,
        "effective_display_fps":effective_fps,"renderer":"production_stream_protocol_v2"})
    );
    if !passed {
        return Err("native stream acceptance failed".into());
    }
    Ok(())
}
