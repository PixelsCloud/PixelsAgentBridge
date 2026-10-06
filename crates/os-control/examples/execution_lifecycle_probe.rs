//! Native containment fixture, including a worker that leaves a child behind.
use pab_os_control::execution::{PreparedUser, channel, current_identity};
use std::{ffi::OsString, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const WAIT: Duration = Duration::from_secs(15);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|v| v == "--sleep") {
        tokio::time::sleep(Duration::from_secs(120)).await;
        return Ok(());
    }
    if args.first().is_some_and(|v| v == "--worker") {
        let mut stream = channel::connect(&args[1], args[2].parse()?, WAIT).await?;
        let child = std::process::Command::new(std::env::current_exe()?)
            .arg("--sleep")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => {
                stream.write_u8(1).await?;
                child
            }
            Err(error) => {
                let text = error.to_string();
                stream.write_u8(0).await?;
                stream.write_u32(text.len() as u32).await?;
                stream.write_all(text.as_bytes()).await?;
                return Err(error.into());
            }
        };
        stream.write_u32(child.id()).await?;
        // Explicit handshake ensures the parent acquired its process witness.
        stream.read_u8().await?;
        if args[3] == "exit" {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(120)).await;
        child.kill()?;
        child.wait()?;
        return Ok(());
    }
    let selected: u32 = args.first().ok_or("expected UID / WTS session")?.parse()?;
    #[cfg(unix)]
    let user = PreparedUser::for_uid(selected)?;
    #[cfg(windows)]
    let user = PreparedUser::for_session(selected)?;
    let original = current_identity()?;
    for mode in ["terminate", "drop", "exit"] {
        eprintln!("lifecycle {mode}: bind and spawn");
        let mut listener = channel::WorkerListener::bind(user.identity())?;
        let mut worker = user.spawn(
            &std::env::current_exe()?,
            &[
                OsString::from("--worker"),
                OsString::from(listener.address()),
                OsString::from(std::process::id().to_string()),
                OsString::from(mode),
            ],
            &user.identity().home,
        )?;
        let mut stream = listener.accept(worker.id(), WAIT).await?;
        if tokio::time::timeout(WAIT, stream.read_u8()).await?? != 1 {
            let len = stream.read_u32().await? as usize;
            if len > 2048 {
                return Err("bad fixture error size".into());
            }
            let mut bytes = vec![0; len];
            stream.read_exact(&mut bytes).await?;
            return Err(format!(
                "fixture descendant spawn: {}",
                String::from_utf8_lossy(&bytes)
            )
            .into());
        }
        let child_pid = tokio::time::timeout(WAIT, stream.read_u32()).await??;
        let witness = Witness::new(child_pid)?;
        if !witness.running()? {
            return Err("fixture child did not run".into());
        }
        stream.write_u8(1).await?;
        if mode == "terminate" {
            eprintln!("lifecycle {mode}: stop worker group");
            worker.terminate()?;
        }
        if mode == "exit" {
            let deadline = tokio::time::Instant::now() + WAIT;
            while worker.try_wait()?.is_none() {
                if tokio::time::Instant::now() > deadline {
                    return Err("worker exit timed out".into());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        drop(worker);
        let deadline = tokio::time::Instant::now() + WAIT;
        while witness.running()? {
            if tokio::time::Instant::now() > deadline {
                return Err(format!("descendant {child_pid} survived {mode}").into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(stream);
        drop(listener);
        if current_identity()? != original {
            return Err("service identity changed".into());
        }
        println!(
            "{}",
            serde_json::json!({"mode":mode,"account_id":user.identity().account_id,"descendant_stopped":true,"service_identity_unchanged":true})
        );
    }
    Ok(())
}

#[cfg(unix)]
struct Witness(u32);
#[cfg(unix)]
impl Witness {
    fn new(pid: u32) -> std::io::Result<Self> {
        Ok(Self(pid))
    }
    fn running(&self) -> std::io::Result<bool> {
        let result = std::process::Command::new("/bin/ps")
            .args(["-p", &self.0.to_string(), "-o", "stat="])
            .output()?;
        let state = String::from_utf8_lossy(&result.stdout);
        Ok(result.status.success() && !state.trim().is_empty() && !state.trim().starts_with('Z'))
    }
}
#[cfg(windows)]
struct Witness(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Witness {
    fn new(pid: u32) -> std::io::Result<Self> {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
    fn running(&self) -> std::io::Result<bool> {
        use windows_sys::Win32::{
            Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::WaitForSingleObject,
        };
        match unsafe { WaitForSingleObject(self.0, 0) } {
            WAIT_TIMEOUT => Ok(true),
            WAIT_OBJECT_0 => Ok(false),
            _ => Err(std::io::Error::last_os_error()),
        }
    }
}
#[cfg(windows)]
impl Drop for Witness {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
