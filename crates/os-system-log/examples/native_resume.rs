//! Acceptance helper: resume from a saved native position after a source change.
//! A lost position must be reported as a gap, never as successful exhaustion.

use pab_os_system_log::{LogError, scan};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let source = args.next().ok_or("source required")?;
    let after = args.next().ok_or("native position required")?;
    let start: i64 = args.next().ok_or("start milliseconds required")?.parse()?;
    let end: i64 = args.next().ok_or("end milliseconds required")?.parse()?;
    match scan(&source, Some(&after), start, end, 128, 512 * 1024) {
        Ok(page) => {
            println!(
                "{}",
                serde_json::json!({
                    "status": "ok",
                    "scanned": page.scanned_count,
                    "events": page.events.len(),
                    "exhausted": page.exhausted,
                    "verifiable": page.verifiable,
                })
            );
        }
        Err(LogError::Gap(reason)) => {
            println!("{}", serde_json::json!({"status": "gap", "reason": reason}));
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
