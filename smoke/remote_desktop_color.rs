//! Headless synthetic-only codec oracle. Does not open or capture a desktop.
use pab_desktop_media::software::{SoftwareDecoder, SoftwareEncoder};
use std::{
    io::{Read, Write},
    path::Path,
};
type Error = Box<dyn std::error::Error + Send + Sync>;
fn color(frame: u32, x: usize, y: usize) -> [u8; 3] {
    if y == 0 || y == 17 {
        let v = if ((frame >> (x % 16)) & 1) ^ u32::from(x >= 16) == 1 {
            224
        } else {
            32
        };
        return [v, v, v];
    }
    [
        32 + ((frame * 3 + x as u32 * 29 + y as u32 * 17) % 192) as u8,
        32 + ((frame * 5 + x as u32 * 11 + y as u32 * 31) % 192) as u8,
        32 + ((frame * 7 + x as u32 * 37 + y as u32 * 13) % 192) as u8,
    ]
}
fn inspect(bytes: &[u8], w: usize, h: usize) -> serde_json::Value {
    let pixel = |gx: usize, gy: usize| {
        let x = w * (2 * gx + 1) / 64;
        let y = h * (2 * gy + 1) / 36;
        let uv = w * h + (y / 2) * w + (x / 2) * 2;
        let (l, u, v) = (
            1.16438356 * (f64::from(bytes[y * w + x]) - 16.),
            f64::from(bytes[uv]) - 128.,
            f64::from(bytes[uv + 1]) - 128.,
        );
        let clip = |n: f64| (n + 0.5).floor().clamp(0., 255.) as u8;
        [
            clip(l + 1.79274107 * v),
            clip(l - 0.21324861 * u - 0.53290933 * v),
            clip(l + 2.11240179 * u),
        ]
    };
    let mut pattern = 0;
    for x in 0..16 {
        if pixel(x, 0).iter().map(|v| u32::from(*v)).sum::<u32>() / 3 > 128 {
            pattern |= 1 << x;
        }
    }
    let mut worst = serde_json::Value::Null;
    let mut max = 0;
    for y in 0..18 {
        for x in 0..32 {
            let actual = pixel(x, y);
            let expected = color(pattern, x, y);
            let error = actual
                .into_iter()
                .zip(expected)
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap();
            if error > max {
                max = error;
                worst = serde_json::json!({"cell":[x,y],"actual":actual,"expected":expected});
            }
        }
    }
    serde_json::json!({"pattern":pattern,"max_error":max,"worst":worst})
}
fn main() -> Result<(), Error> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 || !matches!(args[0].as_str(), "generate" | "replay") {
        return Err(
            "usage: remote_desktop_color generate SYNTHETIC_ARCHIVE | replay ARCHIVE".into(),
        );
    }
    let mut report = Vec::new();
    if args[0] == "generate" {
        let path = Path::new(&args[1]);
        if !path.components().any(|c| c.as_os_str() == "smoke") {
            return Err("generated archive must be under smoke".into());
        }
        let (w, h) = (1920usize, 1080usize);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(b"PABPROOF1")?;
        for n in [w as u32, h as u32, 30] {
            file.write_all(&n.to_le_bytes())?;
        }
        let mut encoder = SoftwareEncoder::new(w, h, 30)?;
        let mut decoder = SoftwareDecoder::new(w, h)?;
        let mut pixels = vec![0u8; w * h * 4];
        for (index, frame) in (140..=203)
            .chain(205..=211)
            .chain(std::iter::once(213))
            .enumerate()
        {
            for gy in 0..18 {
                for gx in 0..32 {
                    let rgb = color(frame, gx, gy);
                    let bgra = [rgb[2], rgb[1], rgb[0], 255];
                    for y in gy * h / 18..(gy + 1) * h / 18 {
                        for p in pixels[(y * w + gx * w / 32) * 4..(y * w + (gx + 1) * w / 32) * 4]
                            .chunks_exact_mut(4)
                        {
                            p.copy_from_slice(&bgra);
                        }
                    }
                }
            }
            let packet = encoder.encode_bgra(&pixels, w * 4)?;
            file.write_all(&(packet.len() as u32).to_le_bytes())?;
            file.write_all(&(index as u64).to_le_bytes())?;
            file.write_all(packet)?;
            let decoded = decoder
                .decode(packet)?
                .ok_or("software decoder buffered a frame")?;
            let mut row = inspect(decoded, w, h);
            row["frame"] = (index + 1).into();
            row["bytes"] = packet.len().into();
            if row["pattern"] != frame {
                return Err("software decoded a stale/mixed source marker".into());
            }
            report.push(row);
        }
        file.flush()?;
    } else {
        let mut file = std::fs::File::open(&args[1])?;
        if file.metadata()?.len() > 64 * 1024 * 1024 {
            return Err("archive too large".into());
        }
        let mut header = [0u8; 21];
        file.read_exact(&mut header)?;
        if &header[..9] != b"PABPROOF1" {
            return Err("invalid archive header".into());
        }
        let w = u32::from_le_bytes(header[9..13].try_into()?) as usize;
        let h = u32::from_le_bytes(header[13..17].try_into()?) as usize;
        if w % 2 != 0 || h % 2 != 0 || w < 32 || h < 18 {
            return Err("proof oracle requires even dimensions and complete cells".into());
        }
        let mut decoder = SoftwareDecoder::new(w, h)?;
        loop {
            let mut data = [0u8; 12];
            let count = file.read(&mut data[..1])?;
            if count == 0 {
                break;
            }
            file.read_exact(&mut data[1..])?;
            let size = u32::from_le_bytes(data[..4].try_into()?) as usize;
            let index = u64::from_le_bytes(data[4..].try_into()?);
            if size == 0 || size > 16 * 1024 * 1024 || index != report.len() as u64 {
                return Err("invalid packet size/sequence".into());
            }
            let mut packet = vec![0; size];
            file.read_exact(&mut packet)?;
            let decoded = decoder
                .decode(&packet)?
                .ok_or("software decoder buffered a frame")?;
            let mut row = inspect(decoded, w, h);
            row["frame"] = (index + 1).into();
            row["bytes"] = size.into();
            report.push(row);
        }
    }
    let max = report
        .iter()
        .map(|r| r["max_error"].as_u64().unwrap())
        .max()
        .ok_or("empty archive")?;
    println!(
        "{}",
        serde_json::json!({"desktop_access":false,"max_error":max,"threshold":16,"within_threshold":max<=16,"frames":report})
    );
    Ok(())
}
