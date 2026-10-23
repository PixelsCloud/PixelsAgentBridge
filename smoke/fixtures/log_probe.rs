//! Read-only parser probe for an installed Pixels log or an exact private copy.
//! Prints counts/hashes, never log contents. Does not change the running service.
use sha2::{Digest, Sha256};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = args.next().ok_or("log root required")?;
    let start: i64 = args
        .next()
        .ok_or("start Unix milliseconds required")?
        .parse()?;
    let end: i64 = args
        .next()
        .ok_or("end Unix milliseconds required")?
        .parse()?;
    if args.next().is_some() || start >= end {
        return Err("expected log-root start-ms end-ms".into());
    }
    let mut cursor = None;
    let mut events = 0_usize;
    let mut scanned = 0_u64;
    let mut digest = Sha256::new();
    for page_number in 1..=4096 {
        let page = pab_os_system_log::pixels::scan(
            Path::new(&root),
            "pixels-file:executor.log",
            cursor.as_deref(),
            start,
            end,
            128,
            256 * 1024,
        )?;
        scanned += page.scanned_count;
        events += page.events.len();
        for event in page.events {
            digest.update(event.raw);
        }
        if page.exhausted {
            println!(
                "{}",
                serde_json::json!({"events":events,"scanned":scanned,
                "pages":page_number,"exhausted":true,"raw_sha256":format!("{:x}",digest.finalize())})
            );
            if events == 0 {
                return Err("no events in requested interval".into());
            }
            return Ok(());
        }
        if cursor == page.next_native_position {
            return Err("cursor did not advance".into());
        }
        cursor = page.next_native_position;
    }
    Err("page budget exhausted".into())
}
