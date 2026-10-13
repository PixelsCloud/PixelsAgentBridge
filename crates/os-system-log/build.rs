fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/macos_oslog_store.m")
            .flag("-fobjc-arc")
            .compile("pab_oslog_store");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=OSLog");
        println!("cargo:rerun-if-changed=src/macos_oslog_store.m");
    }
}
