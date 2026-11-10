//! Same large-file acceptance client, built separately while a roundtrip runs.
#[path = "large_file_transfer.rs"]
mod large_file_transfer;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    large_file_transfer::main()
}
