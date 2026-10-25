fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("media-priority-parent-check") {
        return priority_parent_check();
    }
    std::process::ExitCode::from(pab_desktop_media::worker_main(std::env::args().skip(1)) as u8)
}

// Standalone no-GPU test fixture. Keep the isolated parent outside libtest so
// the normal smoke suite contains no intentionally ignored child test.
#[cfg(windows)]
fn priority_parent_check() -> std::process::ExitCode {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetPriorityClass(process: isize) -> u32;
    }
    // SAFETY: this process's pseudo handle is valid and must not be closed.
    if unsafe { GetPriorityClass(GetCurrentProcess()) } != 0x00004000 {
        return std::process::ExitCode::FAILURE;
    }
    let Ok(executable) = std::env::current_exe() else {
        return std::process::ExitCode::FAILURE;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return std::process::ExitCode::FAILURE;
    };
    let backend =
        pab_desktop_media::NativeBackend::new(executable, vec!["media-priority-check".into()]);
    use pab_remote_desktop::DesktopBackend;
    match runtime.block_on(backend.screens()) {
        Ok(screens) if screens.len() == 1 && screens[0].id == "media-priority-fixture" => {
            println!("media priority verified background->normal");
            std::process::ExitCode::SUCCESS
        }
        _ => std::process::ExitCode::FAILURE,
    }
}
