//! E2 IPC fixture: run under the service account with a UID / WTS session ID.
use pab_os_control::execution::{PreparedUser, UserIdentity, channel, current_identity};
use std::{ffi::OsString, io, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const WAIT: Duration = Duration::from_secs(15);
const BYTES: usize = 300 * 1024 + 1;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|v| v == "--worker") {
        let mut stream = channel::connect(&args[1], args[2].parse()?, WAIT).await?;
        let identity = serde_json::to_vec(&current_identity()?)?;
        stream.write_u32(identity.len() as u32).await?;
        stream.write_all(&identity).await?;
        let mut buffer = [0; 8192];
        let mut offset = 0;
        while offset < BYTES {
            let size = (BYTES - offset).min(buffer.len());
            stream.read_exact(&mut buffer[..size]).await?;
            if buffer[..size]
                .iter()
                .enumerate()
                .any(|(i, b)| *b != ((offset + i) % 251) as u8)
            {
                return Err("binary payload mismatch".into());
            }
            offset += size;
        }
        stream.write_u8(1).await?;
        if tokio::time::timeout(WAIT, stream.read_u8())
            .await?
            .unwrap_err()
            .kind()
            != io::ErrorKind::UnexpectedEof
        {
            return Err("parent close did not reach worker".into());
        }
        return Ok(());
    }
    let selected: u32 = args.first().ok_or("expected UID or WTS session")?.parse()?;
    #[cfg(windows)]
    let prepared = PreparedUser::for_session(selected)?;
    #[cfg(unix)]
    let prepared = PreparedUser::for_uid(selected)?;
    let parent = current_identity()?;
    let mut listener = channel::WorkerListener::bind(prepared.identity())?;
    let address = listener.address().to_owned();
    // This connects from the wrong PID before the intended child. The listener
    // must reject it and still accept the real worker within the same deadline.
    let interloper = channel::connect(&address, std::process::id(), WAIT).await?;
    drop(interloper);
    let mut child = prepared.spawn(
        &std::env::current_exe()?,
        &[
            OsString::from("--worker"),
            OsString::from(&address),
            OsString::from(std::process::id().to_string()),
        ],
        &prepared.identity().home,
    )?;
    let mut stream = listener.accept(child.id(), WAIT).await?;
    let actual = tokio::time::timeout(WAIT, async {
        let size = stream.read_u32().await? as usize;
        if size > 16 * 1024 {
            return Err(io::Error::other("identity envelope too large"));
        }
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).await?;
        serde_json::from_slice::<UserIdentity>(&bytes).map_err(io::Error::other)
    })
    .await??;
    if &actual != prepared.identity() {
        return Err("native worker identity mismatch".into());
    }
    let payload = (0..BYTES).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    tokio::time::timeout(WAIT, stream.write_all(&payload)).await??;
    if tokio::time::timeout(WAIT, stream.read_u8()).await?? != 1 {
        return Err("missing binary ack".into());
    }
    drop(stream);
    let end = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(code) = child.try_wait()? {
            if code != 0 {
                return Err(format!("worker exited {code}").into());
            }
            break;
        }
        if tokio::time::Instant::now() >= end {
            return Err("worker did not stop on channel close".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(listener);
    if current_identity()? != parent {
        return Err("parent identity changed".into());
    }
    #[cfg(unix)]
    if std::path::Path::new(&address).exists() {
        return Err("socket not cleaned up".into());
    }
    #[cfg(windows)]
    if channel::connect(&address, std::process::id(), WAIT)
        .await
        .err()
        .is_none_or(|e| e.kind() != io::ErrorKind::NotFound)
    {
        return Err("pipe endpoint still exists after worker shutdown".into());
    }
    println!(
        "{}",
        serde_json::json!({"identity":actual,"parent_identity_unchanged":true,"binary_bytes":BYTES,"wrong_client_rejected":true,"worker_stopped_on_eof":true,"endpoint_cleaned":true})
    );
    Ok(())
}
