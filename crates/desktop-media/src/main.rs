fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(pab_desktop_media::worker_main(std::env::args().skip(1)) as u8)
}
