fn main() {
    // FIXME: It would be nice to run this only when tests are run.
    println!("cargo:rerun-if-changed=tests/pod.c");

    let libs = system_deps::Config::new()
        .probe()
        .expect("Cannot find libspa");
    let libspa = libs.get_by_name("libspa").unwrap();
    // Ubuntu 22.04's 0.3.48 headers predate the raw-video flags field.
    // Probe the actual C layout used by bindgen; feature flags alone do not
    // establish which system headers are installed.
    println!("cargo:rustc-check-cfg=cfg(pab_spa_raw_has_flags)");
    let header = libspa.include_paths.iter()
        .map(|path| path.join("spa/param/video/raw.h"))
        .find(|path| path.is_file()).expect("libspa raw video header is missing");
    println!("cargo:rerun-if-changed={}", header.display());
    let contents = std::fs::read_to_string(header).expect("could not read libspa header");
    if contents.lines().any(|line| line.split("/*").next().unwrap_or("")
        .split_whitespace().last() == Some("flags;")) {
        println!("cargo:rustc-cfg=pab_spa_raw_has_flags");
    }

    cc::Build::new()
        .file("tests/pod.c")
        .shared_flag(true)
        .flag("-Wno-missing-field-initializers")
        .includes(&libspa.include_paths)
        .compile("pod");
}
