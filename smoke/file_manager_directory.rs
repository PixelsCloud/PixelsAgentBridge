//! Isolated index benchmark; never reads user directories or installed apps.
use pab_file_manager::{DirectoryIndex, encode_path};
use pab_protocol::{FileByteCount, FileManagerFilter};
use std::time::Instant;
mod file_manager_fixture;
use file_manager_fixture::cleanup_fixture;

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let count: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "100000".into())
        .parse()?;
    if count > 100000 {
        return Err("fixture entry limit is 100000".into());
    }
    let dir = match std::env::args().nth(2) {
        Some(parent) => tempfile::tempdir_in(parent)?,
        None => tempfile::tempdir()?,
    };
    let files = dir.path().join("files");
    std::fs::create_dir(&files)?;
    let generated = Instant::now();
    for n in 0..count {
        std::fs::File::create(files.join(format!("entry-{n:06}.txt")))?;
    }
    let generation_ms = generated.elapsed().as_millis();
    let index = DirectoryIndex::open(&dir.path().join("index.sqlite3")).await?;
    let (_stop, cancel) = tokio::sync::watch::channel(false);
    let scan = Instant::now();
    let id = index
        .scan("isolated-probe", &encode_path(&files), cancel)
        .await?;
    let scan_ms = scan.elapsed().as_millis();
    let read = Instant::now();
    let filter = FileManagerFilter::default();
    let first = index.read("isolated-probe", id, None, 256, &filter).await?;
    assert_eq!(first.total_entries, FileByteCount(count as u64));
    let first_page_ms = read.elapsed().as_millis();
    // Querying the index must not depend on re-reading the source directory.
    std::fs::rename(&files, dir.path().join("source-renamed"))?;
    let full = Instant::now();
    let mut seen = first.entries.len();
    let mut cursor = first.next_after;
    let mut last = first.entries.last().map(|e| e.name.clone());
    while let Some(after) = cursor {
        let page = index
            .read("isolated-probe", id, Some(&after), 256, &filter)
            .await?;
        for entry in &page.entries {
            if let Some(previous) = &last {
                assert!(previous < &entry.name);
            }
            last = Some(entry.name.clone());
        }
        seen += page.entries.len();
        cursor = page.next_after;
    }
    assert_eq!(seen, count);
    let paging_ms = full.elapsed().as_millis();
    index.release_owner("isolated-probe").await?;
    index.shutdown().await;
    cleanup_fixture(dir).await?;
    println!(
        "{}",
        serde_json::json!({"status":"passed","entries":count,"generation_ms":generation_ms,"scan_ms":scan_ms,"first_page_ms":first_page_ms,"all_remaining_pages_ms":paging_ms,"source_renamed_before_paging":true,"cleanup_complete":true})
    );
    Ok(())
}
