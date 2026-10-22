fn main() {
    // Stable Rust does not make sqlx::migrate! track newly added files. Rebuild
    // the embedded migrator when the directory changes, including additions.
    println!("cargo:rerun-if-changed=migrations");
}
