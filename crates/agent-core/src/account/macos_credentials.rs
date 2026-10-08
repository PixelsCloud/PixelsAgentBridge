//! Apple's stable Keychain client keeps free signing and app upgrades out of an
//! item's executable ACL. Secrets use private pipes, never argv, env or open IPC.
use super::AccountError;
use std::{
    collections::HashMap,
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
const SERVICE: &str = "PixelsAgentBridge.Account";
const NOT_FOUND: i32 = 44; // errSecItemNotFound (-25300) as a process exit status.
fn cache() -> &'static Mutex<HashMap<String, Zeroizing<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Zeroizing<String>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn account(scope: &str, slot: &str) -> Result<String, AccountError> {
    if scope.len() != 64
        || !scope.bytes().all(|b| b.is_ascii_hexdigit())
        || slot.parse::<pab_protocol::RequestId>().is_err()
    {
        return Err(AccountError::Storage);
    }
    Ok(format!("{scope}:{slot}"))
}
pub(super) fn read(scope: &str, slot: &str) -> Result<Option<Zeroizing<String>>, AccountError> {
    let account = account(scope, slot)?;
    if let Some(token) = cache()
        .lock()
        .map_err(|_| AccountError::Storage)?
        .get(&account)
    {
        return Ok(Some(token.clone()));
    }
    let value = read_uncached(&account)?;
    if let Some(token) = &value {
        let mut cache = cache().lock().map_err(|_| AccountError::Storage)?;
        // Immutable credential slots; bound memory after repeated login changes.
        if cache.len() >= 16 {
            cache.clear();
        }
        cache.insert(account, token.clone());
    }
    Ok(value)
}
fn read_uncached(account: &str) -> Result<Option<Zeroizing<String>>, AccountError> {
    let (code, bytes) = run(
        &["find-generic-password", "-s", SERVICE, "-a", account, "-w"],
        None,
    )?;
    if code == NOT_FOUND {
        return Ok(None);
    }
    if code != 0 {
        return Err(AccountError::Storage);
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| AccountError::Storage)?
        .trim_end_matches(['\r', '\n']);
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AccountError::Storage);
    }
    Ok(Some(Zeroizing::new(text.to_owned())))
}
pub(super) fn write(scope: &str, slot: &str, token: &str) -> Result<(), AccountError> {
    let account = account(scope, slot)?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AccountError::Storage);
    }
    // Strict hex/UUID grammar permits no command injection. security -i reads
    // this from stdin. Never use -v (echo), -A (all apps), a shell or argv secrets.
    let input = Zeroizing::new(format!(
        "add-generic-password -s {SERVICE} -a {account} -w {token}\n"
    ));
    let (code, _) = run(&["-i", "-q"], Some(input.as_bytes()))?;
    if code != 0 || read_uncached(&account)?.as_ref().map(|s| s.as_str()) != Some(token) {
        return Err(AccountError::Storage);
    }
    Ok(())
}
pub(super) fn delete(scope: &str, slot: &str) -> Result<(), AccountError> {
    let account = account(scope, slot)?;
    let (code, _) = run(
        &["delete-generic-password", "-s", SERVICE, "-a", &account],
        None,
    )?;
    if !matches!(code, 0 | NOT_FOUND) {
        return Err(AccountError::Storage);
    }
    cache()
        .lock()
        .map_err(|_| AccountError::Storage)?
        .remove(&account);
    Ok(())
}
fn run(args: &[&str], input: Option<&[u8]>) -> Result<(i32, Zeroizing<Vec<u8>>), AccountError> {
    let mut child = Command::new("/usr/bin/security")
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| AccountError::Storage)?;
    let result = (|| {
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or(AccountError::Storage)?
                .write_all(input)
                .map_err(|_| AccountError::Storage)?;
        }
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|_| AccountError::Storage)? {
                break status;
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err(AccountError::Storage);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut bytes = Zeroizing::new(Vec::new());
        child
            .stdout
            .take()
            .ok_or(AccountError::Storage)?
            .take(4096)
            .read_to_end(&mut bytes)
            .map_err(|_| AccountError::Storage)?;
        Ok((status.code().ok_or(AccountError::Storage)?, bytes))
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
