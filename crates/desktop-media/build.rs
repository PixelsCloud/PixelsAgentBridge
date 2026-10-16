fn main() {
    println!("cargo:rerun-if-changed=native");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // One translation unit; no nested parallel build or network downloads.
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include("native/windows")
        .include("native/vendor")
        .file("native/windows/worker.cpp")
        .flag_if_supported("/EHsc")
        .define("UNICODE", None)
        .define("_UNICODE", None)
        .compile("pab_desktop_media");
    for library in [
        "d3d11",
        "dxgi",
        "dxguid",
        "user32",
        "gdi32",
        "mfplat",
        "mfuuid",
        "wmcodecdspuuid",
        "ole32",
        "shcore",
    ] {
        println!("cargo:rustc-link-lib={library}");
    }
}
