#![cfg_attr(windows, windows_subsystem = "windows")]
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Write},
    sync::Mutex as StdMutex,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, tcp::OwnedWriteHalf},
    sync::Mutex,
};
struct Proof {
    ack: Mutex<Option<OwnedWriteHalf>>,
    hash: StdMutex<Sha256>,
    bytes: StdMutex<u64>,
}
fn exchange(port: u16, v: Value) -> Result<Value, String> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    writeln!(s, "{v}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}
#[tauri::command]
async fn start(app: tauri::AppHandle, on_data: tauri::ipc::Channel<Value>) -> Result<(), String> {
    use tauri::Manager;
    let s = TcpStream::connect(("127.0.0.1", 39081))
        .await
        .map_err(|e| e.to_string())?;
    s.set_nodelay(true).map_err(|e| e.to_string())?;
    let (mut read, write) = s.into_split();
    *app.state::<Proof>().ack.lock().await = Some(write);
    let (send_frames, mut frames) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    tauri::async_runtime::spawn(async move {
        loop {
            let n = match read.read_u32_le().await {
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 {
                let _ = send_frames.send(Vec::new()).await;
                break;
            }
            if n > 16384 {
                break;
            }
            let mut bytes = vec![0; n as usize];
            if read.read_exact(&mut bytes).await.is_err() {
                break;
            }
            if send_frames.send(bytes).await.is_err() {
                break;
            }
        }
    });
    tauri::async_runtime::spawn(async move {
        let emit = |bytes: &[u8]| {
            let st = app.state::<Proof>();
            st.hash.lock().unwrap().update(bytes);
            *st.bytes.lock().unwrap() += bytes.len() as u64;
            on_data.send(json!({"data":STANDARD.encode(bytes)}))
        };
        let mut pending = Vec::with_capacity(16384);
        loop {
            let event = if pending.is_empty() {
                frames.recv().await
            } else {
                match tokio::time::timeout(std::time::Duration::from_millis(10), frames.recv())
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        if emit(&pending).is_err() {
                            break;
                        }
                        pending.clear();
                        continue;
                    }
                }
            };
            let Some(bytes) = event.filter(|bytes| !bytes.is_empty()) else {
                if !pending.is_empty() {
                    let _ = emit(&pending);
                }
                let _ = on_data.send(json!({"ended":true}));
                break;
            };
            if pending.len() + bytes.len() > 16384 {
                if emit(&pending).is_err() {
                    break;
                }
                pending.clear();
            }
            pending.extend_from_slice(&bytes);
            if pending.len() == 16384 {
                if emit(&pending).is_err() {
                    break;
                }
                pending.clear();
            }
        }
    });
    Ok(())
}
#[tauri::command]
async fn ack(offset: u64, state: tauri::State<'_, Proof>) -> Result<(), String> {
    let mut g = state.ack.lock().await;
    g.as_mut()
        .ok_or("not started")?
        .write_all(&offset.to_le_bytes())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn control(value: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || exchange(39082, value))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn admin(value: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || exchange(39083, value))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn visibility(window: tauri::WebviewWindow, hidden: bool) -> Result<(), String> {
    if hidden { window.hide() } else { window.show() }.map_err(|e| e.to_string())
}
#[tauri::command]
fn memory() -> Value {
    unsafe {
        use windows_sys::Win32::System::{
            ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
            Threading::GetCurrentProcess,
        };
        let mut m: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        m.cb = std::mem::size_of_val(&m) as u32;
        let ok = K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut m as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            m.cb,
        );
        json!({"ok":ok!=0,"working_set":m.WorkingSetSize,"private_bytes":m.PrivateUsage})
    }
}
#[tauri::command]
fn finish(
    app: tauri::AppHandle,
    state: tauri::State<'_, Proof>,
    mut value: Value,
) -> Result<(), String> {
    value["proof_pid"] = json!(std::process::id());
    value["native_received_sha256"] = json!(format!(
        "{:x}",
        state.hash.lock().unwrap().clone().finalize()
    ));
    value["native_received_bytes"] = json!(*state.bytes.lock().unwrap());
    let dest = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("webview-result.json");
    std::fs::write(dest, serde_json::to_vec_pretty(&value).unwrap()).map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}
#[tauri::command]
fn checkpoint(value: Value) -> Result<(), String> {
    let p = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("webview-progress.json");
    std::fs::write(p, serde_json::to_vec(&value).unwrap()).map_err(|e| e.to_string())
}
fn main() {
    // Only this disposable process; do not repair registry or change installed Pixels.
    unsafe {
        std::env::set_var(
            "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
            r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application\154.0.4258.62",
        );
    }
    tauri::Builder::default()
        .manage(Proof {
            ack: Mutex::new(None),
            hash: StdMutex::new(Sha256::new()),
            bytes: StdMutex::new(0),
        })
        .invoke_handler(tauri::generate_handler![
            start, ack, control, admin, visibility, memory, finish, checkpoint
        ])
        .run(tauri::generate_context!())
        .expect("isolated proof window failed");
}
