//! Disposable parser process. The Executor owns its deadline and can kill it
//! without affecting command, browser, or file operations.
use pab_document::{OcrMode, extract_path, render_page};
use serde::{Deserialize, Serialize};
use std::{io::{Read, Write}, path::PathBuf, process::ExitCode};

// Native parsers and OCR run only in this short-lived process. Bound its
// memory as well as the input, output and wall-clock budgets imposed by
// the Executor. A failed limit is fatal, not a reason to parse uncontained.
// PDFium on Linux reserves substantially more virtual address space than it
// commits. RLIMIT_AS counts those reservations, so allow 2 GiB there while
// retaining the tighter Windows Job Object cap.
#[cfg(target_os = "linux")]
const MEMORY_BUDGET: usize = 2 * 1024 * 1024 * 1024;
#[cfg(not(target_os = "linux"))]
const MEMORY_BUDGET: usize = 1536 * 1024 * 1024;

#[cfg(windows)]
fn limit_memory() -> std::io::Result<()> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            JobObjects::{AssignProcessToJobObject, CreateJobObjectW,
                JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
                SetInformationJobObject},
            Threading::GetCurrentProcess,
        },
    };
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() { return Err(std::io::Error::last_os_error()); }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    limits.ProcessMemoryLimit = MEMORY_BUDGET;
    let configured = unsafe { SetInformationJobObject(job, JobObjectExtendedLimitInformation,
        (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
        std::mem::size_of_val(&limits) as u32) };
    let assigned = if configured != 0 {
        unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) }
    } else { 0 };
    if assigned == 0 {
        let error = std::io::Error::last_os_error();
        unsafe { CloseHandle(job); }
        return Err(error);
    }
    // Keep the job alive until process exit; the OS closes this handle then.
    Ok(())
}

#[cfg(target_os = "linux")]
fn limit_memory() -> std::io::Result<()> {
    let budget = libc::rlimit { rlim_cur: MEMORY_BUDGET as libc::rlim_t,
        rlim_max: MEMORY_BUDGET as libc::rlim_t };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &budget) } == 0 { Ok(()) }
    else { Err(std::io::Error::last_os_error()) }
}

#[cfg(target_os = "macos")]
fn limit_memory() -> std::io::Result<()> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    // macOS rejects RLIMIT_AS and RLIMIT_DATA on supported test hosts. Keep a
    // watchdog outside the current-thread parser runtime so native extraction
    // cannot block the memory check. The Executor still owns the time limit.
    std::thread::Builder::new().name("document-memory-watchdog".into()).spawn(|| {
        let pid = Pid::from_u32(std::process::id());
        let mut system = System::new();
        loop {
            system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true,
                ProcessRefreshKind::nothing().with_memory());
            if system.process(pid).is_some_and(|process| process.memory() > MEMORY_BUDGET as u64) {
                std::process::exit(137);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    })?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    path: PathBuf,
    ocr_mode: OcrMode,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Reply {
    Success { document: pab_document::Document },
    Failure { error: String },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    if let Err(error) = limit_memory() {
        eprintln!("document worker memory limit failed: {error}");
        return ExitCode::FAILURE;
    }
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--render-output") {
        if args.len() != 6 { return ExitCode::FAILURE; }
        let (Some(hash), Some(page), Some(dimension)) = (args[2].to_str(), args[3].to_str(), args[4].to_str())
            else { return ExitCode::FAILURE; };
        let (Ok(page), Ok(dimension)) = (page.parse::<u32>(), dimension.parse::<u16>())
            else { return ExitCode::FAILURE; };
        let Ok(png) = render_page(&PathBuf::from(&args[1]), hash, page, dimension) else { return ExitCode::FAILURE; };
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let Ok(mut file) = options.open(PathBuf::from(&args[5])) else { return ExitCode::FAILURE; };
        if file.write_all(&png).is_err() || file.sync_all().is_err() { return ExitCode::FAILURE; }
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|arg| arg == "--output") {
        if args.len() != 4 { return ExitCode::FAILURE; }
        let path = PathBuf::from(&args[1]);
        let mode = match args[2].to_str() {
            Some("auto") => OcrMode::Auto,
            Some("off") => OcrMode::Off,
            Some("on") => OcrMode::On,
            _ => return ExitCode::FAILURE,
        };
        let result = match extract_path(&path, mode).await {
            Ok(document) => Reply::Success { document },
            Err(error) => Reply::Failure { error: error.to_string() },
        };
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let Ok(mut file) = options.open(PathBuf::from(&args[3])) else { return ExitCode::FAILURE; };
        if serde_json::to_writer(&mut file, &result).is_err() || file.sync_all().is_err() {
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }
    if !args.is_empty() { return ExitCode::FAILURE; }
    let mut input = String::new();
    if std::io::stdin().take(8193).read_to_string(&mut input).is_err() || input.len() > 8192 {
        return ExitCode::FAILURE;
    }
    let result = match serde_json::from_str::<Request>(&input) {
        Ok(request) => match extract_path(&request.path, request.ocr_mode).await {
            Ok(document) => Reply::Success { document },
            Err(error) => Reply::Failure { error: error.to_string() },
        },
        Err(error) => Reply::Failure { error: format!("invalid_request: {error}") },
    };
    if serde_json::to_writer(std::io::stdout().lock(), &result).is_err() {
        return ExitCode::FAILURE;
    }
    let _ = std::io::stdout().flush();
    ExitCode::SUCCESS
}
