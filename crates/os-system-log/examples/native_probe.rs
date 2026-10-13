//! Read a fixed native range for cross-platform acceptance comparisons.

use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let source = args.next().ok_or("source required")?;
    let start: i64 = args.next().ok_or("start milliseconds required")?.parse()?;
    let end: i64 = args.next().ok_or("end milliseconds required")?.parse()?;
    let marker = args.next().ok_or("marker required")?;
    let mut after = None;
    let mut scanned = 0_u64;
    let mut matches = Vec::new();
    let mut exhausted = false;
    for _ in 0..500 {
        let page = pab_os_system_log::scan(&source, after.as_deref(), start, end, 128, 512 * 1024)?;
        scanned += page.scanned_count;
        for event in page.events {
            if event
                .raw
                .windows(marker.len())
                .any(|window| window == marker.as_bytes())
            {
                matches.push(serde_json::json!({
                    "timestamp_unix_ms": event.timestamp_unix_ms,
                    "native_position": event.native_position,
                    "raw_bytes": event.raw.len(),
                    "sha256": format!("{:x}", Sha256::digest(&event.raw)),
                }));
            }
        }
        if page.exhausted {
            exhausted = true;
            break;
        }
        if page.next_native_position == after {
            return Err("native scan made no progress".into());
        }
        after = page.next_native_position;
    }
    println!(
        "{}",
        serde_json::json!({
            "source": source,
            "scanned": scanned,
            "exhausted": exhausted,
            "matches": matches,
        })
    );
    Ok(())
}
