//! Opt-in native encoder interoperability check using generated quadrants.
//! This measures encode/decode throughput, not actual desktop or display FPS.
//! No frame bytes are written to disk. Run: native_capture_check WORKER 15|30 [MODE].
use pab_desktop_media::{NativeBackend, software::SoftwareDecoder};
use pab_protocol::DESKTOP_MAX_FRAME_BYTES;
use pab_remote_desktop::DesktopBackend;
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
    if !(2..=3).contains(&args.len()) {
        return Err("usage: native_capture_check WORKER 15|30 [synthetic mode]".into());
    }
    let mode = args.get(2).map(String::as_str).unwrap_or("synthetic");
    if !matches!(
        mode,
        "synthetic"
            | "synthetic-software"
            | "synthetic-fail-encoder-init"
            | "synthetic-fail-encoder"
    ) {
        return Err("only generated diagnostic sources are allowed".into());
    }
    let fps: u16 = args[1].parse()?;
    if !matches!(fps, 15 | 30) {
        return Err("expected 15 or 30 FPS".into());
    }
    let executable = PathBuf::from(&args[0]);
    let screen = NativeBackend::new(executable.clone(), vec![])
        .screens()
        .await?
        .into_iter()
        .find(|screen| screen.primary)
        .ok_or("missing primary screen")?;
    let (width, height) = (screen.pixel_width as usize, screen.pixel_height as usize);
    let mut decoder = SoftwareDecoder::new(width, height)?;
    let mut command = Command::new(&executable);
    command
        .args(["capture", &screen.id, &fps.to_string(), mode])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut source = command.spawn()?;
    let mut input = source.stdin.take().unwrap();
    let mut output = source.stdout.take().unwrap();
    if output.read_u32_le().await? != 0x31424150
        || output.read_u32_le().await? != screen.pixel_width
        || output.read_u32_le().await? != screen.pixel_height
    {
        return Err("source geometry mismatch".into());
    }
    let mut start = Instant::now();
    let mut last_capture = 0;
    let mut maximum_error = 0;
    let total = u64::from(fps) * 6;
    for index in 0..total {
        input.write_u8(1).await?;
        let size = output.read_u32_le().await? as usize;
        if size == 0 || size > DESKTOP_MAX_FRAME_BYTES || output.read_u64_le().await? != index {
            return Err("source packet metadata mismatch".into());
        }
        let sampled = output.read_u64_le().await?;
        let key = output.read_u32_le().await?;
        if sampled < last_capture || key > 1 || (index == 0 && key != 1) {
            return Err("source timestamp/keyframe mismatch".into());
        }
        if mode == "synthetic-fail-encoder" && index == 2 && key != 1 {
            return Err("encoder fallback must start with an independently decodable frame".into());
        }
        last_capture = sampled;
        let mut packet = vec![0; size];
        output.read_exact(&mut packet).await?;
        let image = decoder
            .decode(&packet)?
            .ok_or("decoder delayed a low-latency frame")?;
        for q in 0..4usize {
            let r = (24 + (index * 3 + q as u64 * 43) % 208) as f64;
            let g = (24 + (index * 7 + q as u64 * 67) % 208) as f64;
            let b = (24 + (index * 11 + q as u64 * 97) % 208) as f64;
            let expected = [
                (16.0 + (0.2126 * r + 0.7152 * g + 0.0722 * b) * 219.0 / 255.0).round() as u8,
                (128.0 + (-0.114572 * r - 0.385428 * g + 0.5 * b) * 224.0 / 255.0).round() as u8,
                (128.0 + (0.5 * r - 0.454153 * g - 0.045847 * b) * 224.0 / 255.0).round() as u8,
            ];
            let x = (q % 2) * width / 2 + width / 4;
            let y = (q / 2) * height / 2 + height / 4;
            let uv = width * height + (y / 2) * width + (x / 2) * 2;
            for (actual, wanted) in [image[y * width + x], image[uv], image[uv + 1]]
                .into_iter()
                .zip(expected)
            {
                maximum_error = maximum_error.max(actual.abs_diff(wanted));
                if maximum_error > 8 {
                    return Err(format!("stale/color error at frame {index}, quadrant {q}").into());
                }
            }
        }
        if index + 1 == u64::from(fps) {
            start = Instant::now();
        }
    }
    let rate = (total - u64::from(fps)) as f64 / start.elapsed().as_secs_f64();
    input.write_u8(0).await?;
    if !source.wait().await?.success() {
        return Err("native source failed on shutdown".into());
    }
    let passed = rate > 12.0 && rate >= f64::from(fps) * 0.9;
    println!(
        "{}",
        serde_json::json!({"passed":passed,"source":mode,"dimensions":[width,height],
        "target_fps":fps,"encode_decode_fps":rate,"decoded":total,"pixel_checks":total*4,"max_yuv_error":maximum_error})
    );
    if !passed {
        return Err("native encode/decode throughput below threshold".into());
    }
    Ok(())
}
