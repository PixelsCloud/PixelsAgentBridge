//! Inspect native source availability without changing channel configuration.

use std::collections::BTreeMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sources = pab_os_system_log::sources("native inventory probe")?;
    let mut counts = BTreeMap::<String, usize>::new();
    let mut examples = BTreeMap::<String, String>::new();
    for source in &sources {
        *counts.entry(source.availability.clone()).or_default() += 1;
        examples
            .entry(source.availability.clone())
            .or_insert_with(|| source.source_id.clone());
    }
    let selected = sources
        .iter()
        .filter(|source| {
            source.source_id == "eventlog:Application" || source.source_id == "eventlog:AMSI/Debug"
        })
        .map(|source| {
            serde_json::json!({
                "source_id": source.source_id,
                "availability": source.availability,
                "reason": source.reason,
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::json!({"counts": counts, "examples": examples, "selected": selected})
    );
    Ok(())
}
