//! Explicit CPU codec benchmark; does not measure capture, display or network FPS.
use pab_desktop_media::software::{SoftwareDecoder, SoftwareEncoder};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let width: usize = std::env::args().nth(1).unwrap_or("1920".into()).parse()?;
    let height = width * 9 / 16;
    let solid = std::env::args().nth(2).as_deref() == Some("solid");
    let mut encoder = SoftwareEncoder::new(width, height, 15)?;
    let mut decoder = SoftwareDecoder::new(width, height)?;
    let mut bgra = vec![0; width * height * 4];
    let mut encode = 0.;
    let mut decode = 0.;
    let mut bytes = 0;
    for index in 0..40 {
        for (i, pixel) in bgra.chunks_exact_mut(4).enumerate() {
            if solid {
                pixel.copy_from_slice(&[(index * 5) as u8, (index * 3) as u8, index as u8, 255]);
            } else {
                pixel.copy_from_slice(&[
                    (i % width / 8 + index * 5) as u8,
                    (i / width / 8 + index * 3) as u8,
                    (i % width / 16 + index) as u8,
                    255,
                ]);
            }
        }
        let start = Instant::now();
        let packet = encoder.encode_bgra(&bgra, width * 4)?;
        let encoded = Instant::now();
        if decoder.decode(packet)?.is_none() {
            return Err("decoder delayed output".into());
        }
        if index >= 10 {
            encode += (encoded - start).as_secs_f64();
            decode += encoded.elapsed().as_secs_f64();
            bytes += packet.len();
        }
    }
    println!(
        "{}",
        serde_json::json!({"dimensions":[width,height], "frames":30,
        "conversion_encode_ms": encode * 1000. / 30., "decode_ms":decode * 1000. / 30., "bytes":bytes})
    );
    Ok(())
}
