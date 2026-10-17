//! Diagnose color drift with generated pixels and the production CPU codec.
//! This is an observation tool, not a capture/display performance benchmark.
use pab_desktop_media::software::{SoftwareDecoder, SoftwareEncoder};
use serde_json::json;
use std::collections::BTreeMap;

fn rgb(y: u8, u: u8, v: u8) -> [f64; 3] {
    // Independent limited-range BT.709 inverse; no dependency RGB helper.
    let y = (f64::from(y) - 16.) * 255. / 219.;
    let cb = (f64::from(u) - 128.) * 255. / 224.;
    let cr = (f64::from(v) - 128.) * 255. / 224.;
    [
        y + 1.5748 * cr,
        y - 0.187324 * cb - 0.468124 * cr,
        y + 1.8556 * cb,
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (width, height) = (1280, 800);
    let green: u8 = std::env::args().nth(1).unwrap_or("81".into()).parse()?;
    let mut runs = Vec::new();
    for history in ["fresh", "black", "white", "pattern", "fade"] {
        let mut encoder = SoftwareEncoder::new(width, height, 15)?;
        let mut decoder = SoftwareDecoder::new(width, height)?;
        let mut source = vec![0; width * height * 4];
        let warmup = if history == "fresh" {
            0
        } else if history == "fade" {
            20
        } else {
            8
        };
        let mut frames = Vec::new();
        for index in 0..warmup + 12 {
            for (i, p) in source.chunks_exact_mut(4).enumerate() {
                let color = if index >= warmup {
                    [74, green, 20, 255]
                } else {
                    match history {
                        "black" => [0, 0, 0, 255],
                        "white" => [255, 255, 255, 255],
                        "fade" => {
                            let mut color = [0, 0, 0, 255];
                            for (channel, target) in [74, green, 20].into_iter().enumerate() {
                                color[channel] =
                                    ((usize::from(target) * index + 240 * (warmup - index))
                                        / warmup) as u8;
                            }
                            color
                        }
                        _ => [
                            (i % width / 8) as u8,
                            (i / width / 8) as u8,
                            (index * 17) as u8,
                            255,
                        ],
                    }
                };
                p.copy_from_slice(&color);
            }
            let packet = encoder.encode_bgra(&source, width * 4)?;
            let nv12 = decoder
                .decode(packet)?
                .ok_or("delayed output in low-latency observation")?;
            if index < warmup {
                continue;
            }
            let mut counts = BTreeMap::<[u8; 3], usize>::new();
            let mut sum = [0.; 3];
            for y in 0..height {
                for x in 0..width {
                    let uv = width * height + y / 2 * width + x / 2 * 2;
                    let value = [nv12[y * width + x], nv12[uv], nv12[uv + 1]];
                    *counts.entry(value).or_default() += 1;
                    let actual = rgb(value[0], value[1], value[2]);
                    for channel in 0..3 {
                        sum[channel] += actual[channel];
                    }
                }
            }
            let means = sum.map(|v| v / (width * height) as f64);
            frames.push(json!({"index":index-warmup,"packet_bytes":packet.len(),"mean_rgb":means,
                "yuv_counts":counts.into_iter().map(|(yuv,count)|json!({"yuv":yuv,"count":count})).collect::<Vec<_>>()}));
        }
        runs.push(json!({"history":history,"frames":frames}));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"dimensions":[width,height],
        "source_rgb":[20,green,74],"pipeline":"production software encoder/decoder; analytic BT.709 inverse, no display", "runs":runs}))?
    );
    Ok(())
}
