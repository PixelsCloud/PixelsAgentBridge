//! Read-only acceptance helper: enumerate exact native records carrying a
//! caller-supplied marker so an independent OS reader can compare them.
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let marker = arguments.next().ok_or("marker argument required")?;
    let requested_source = arguments.next();
    let start_override = arguments
        .next()
        .map(|value| value.parse::<i64>())
        .transpose()?;
    let source = pab_os_system_log::sources("acceptance")?
        .into_iter()
        .find(|source| {
            source.availability == "readable"
                && requested_source
                    .as_ref()
                    .is_none_or(|id| &source.source_id == id)
        })
        .ok_or("no readable source")?;
    let end = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64 + 60_000;
    let start = start_override.unwrap_or(end - 6 * 60_000);
    let mut after = None;
    let mut total = 0_u64;
    let mut found = Vec::new();
    let mut exhausted = false;
    for _ in 0..100 {
        let page = pab_os_system_log::scan(
            &source.source_id,
            after.as_deref(),
            start,
            end,
            128,
            1024 * 1024,
        )?;
        total += page.scanned_count;
        for record in page.events {
            if record
                .message
                .as_deref()
                .is_some_and(|text| text.contains(&marker))
            {
                found.push(serde_json::json!({
                    "native_position": record.native_position,
                    "message": record.message,
                    "timestamp_unix_ms": record.timestamp_unix_ms,
                    "raw_bytes": record.raw.len(),
                }));
            }
        }
        after = page.next_native_position;
        exhausted = page.exhausted;
        if exhausted {
            break;
        }
    }
    println!(
        "{}",
        serde_json::json!({"source":source.source_id,"scanned":total,"exhausted":exhausted,"matches":found})
    );
    Ok(())
}
