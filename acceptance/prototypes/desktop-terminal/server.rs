//! Disposable proof. Separate from every installed Pixels terminal implementation.
#![cfg_attr(windows, windows_subsystem = "windows")]
use base64::{Engine, engine::general_purpose::STANDARD};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const WINDOW: u64 = 256 * 1024;
const CHUNK: usize = 16 * 1024;
#[derive(Default)]
struct Credit {
    produced: u64,
    acked: u64,
    max: u64,
    digest: Sha256,
}

fn main() {
    if let Err(e) = run() {
        if let Some(dir) = std::env::args().nth(2) {
            let _ = std::fs::write(PathBuf::from(dir).join("error.txt"), e.to_string());
        }
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().ok_or("missing port")?.parse()?;
    let dir = PathBuf::from(args.next().ok_or("missing directory")?);
    std::fs::create_dir_all(&dir)?;
    let output = TcpListener::bind(("127.0.0.1", port))?;
    let control = TcpListener::bind(("127.0.0.1", port + 1))?;
    let admin = TcpListener::bind(("127.0.0.1", port + 2))?;
    let pair = NativePtySystem::default().openpty(PtySize {
        cols: 110,
        rows: 32,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut cmd = CommandBuilder::new("powershell.exe");
    // The disposable fixture must not save its synthetic commands to user history.
    cmd.args(["-NoLogo", "-NoExit", "-Command", "if (Get-Command Set-PSReadLineOption -ErrorAction SilentlyContinue) { Set-PSReadLineOption -HistorySaveStyle SaveNothing }"]);
    cmd.cwd(std::env::var_os("USERPROFILE").ok_or("USERPROFILE missing")?);
    let child = pair.slave.spawn_command(cmd)?;
    let child_pid = child.process_id();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let master = Arc::new(Mutex::new(pair.master));
    let child = Arc::new(Mutex::new(child));
    let credit = Arc::new((Mutex::new(Credit::default()), Condvar::new()));
    let stopped = Arc::new(AtomicBool::new(false));
    let ended = Arc::new(AtomicBool::new(false));
    std::fs::write(dir.join("ready.json"), json!({"server_pid":std::process::id(),"shell_pid":child_pid,
        "ports":[port,port+1,port+2],"user":std::env::var("USERNAME").ok(),"home":std::env::var("USERPROFILE").ok(),
        "credit_window":WINDOW,"chunk":CHUNK}).to_string())?;
    let (input_tx, input_rx) = mpsc::sync_channel::<(Vec<u8>, mpsc::SyncSender<String>)>(4);
    thread::spawn(move || {
        while let Ok((bytes, answer)) = input_rx.recv() {
            let result = writer.write_all(&bytes).and_then(|_| writer.flush());
            let _ = answer.send(
                result
                    .map(|_| "applied".into())
                    .unwrap_or_else(|e| e.to_string()),
            );
        }
    });
    let m = master.clone();
    thread::spawn(move || {
        for stream in control.incoming().flatten() {
            let tx = input_tx.clone();
            let m = m.clone();
            thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut br = BufReader::new(stream);
                let mut line = String::new();
                if br.by_ref().take(8193).read_line(&mut line).is_err() || line.len() > 8192 {
                    return;
                }
                let v: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let reply = match v["action"].as_str() {
                    Some("input") => {
                        let bytes = STANDARD
                            .decode(v["data"].as_str().unwrap_or(""))
                            .unwrap_or_default();
                        let (answer, rx) = mpsc::sync_channel(1);
                        if bytes.is_empty() || bytes.len() > 4096 {
                            json!({"error":"invalid_input"})
                        } else if tx.try_send((bytes, answer)).is_err() {
                            json!({"error":"input_backpressure"})
                        } else {
                            json!({"result":rx.recv_timeout(Duration::from_secs(2)).unwrap_or("writer_blocked".into())})
                        }
                    }
                    Some("resize") => {
                        json!({"result":m.lock().unwrap().resize(PtySize{cols:120,rows:35,pixel_width:0,pixel_height:0}).map(|_|"applied".to_string()).unwrap_or_else(|e|e.to_string())})
                    }
                    _ => json!({"error":"unknown_control"}),
                };
                let _ = writeln!(br.get_mut(), "{reply}");
            });
        }
    });
    let c = credit.clone();
    let e = ended.clone();
    let stop = stopped.clone();
    thread::spawn(move || {
        let Ok((mut stream, _)) = output.accept() else {
            return;
        };
        let _ = stream.set_nodelay(true);
        let Ok(mut acknowledgments) = stream.try_clone() else {
            return;
        };
        let ac = c.clone();
        let astop = stop.clone();
        thread::spawn(move || {
            let mut bytes = [0; 8];
            while acknowledgments.read_exact(&mut bytes).is_ok() {
                let value = u64::from_le_bytes(bytes);
                let (lock, wake) = &*ac;
                let mut cr = lock.lock().unwrap();
                if value < cr.acked || value > cr.produced {
                    astop.store(true, Ordering::SeqCst);
                    wake.notify_all();
                    break;
                }
                cr.acked = value;
                wake.notify_all();
            }
        });
        let mut bytes = [0; CHUNK];
        loop {
            {
                let (lock, wake) = &*c;
                let mut cr = lock.lock().unwrap();
                while cr.produced - cr.acked + CHUNK as u64 > WINDOW && !stop.load(Ordering::SeqCst)
                {
                    cr = wake.wait(cr).unwrap();
                }
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let n = match reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            {
                let mut cr = c.0.lock().unwrap();
                cr.produced += n as u64;
                cr.max = cr.max.max(cr.produced - cr.acked);
                cr.digest.update(&bytes[..n]);
            }
            if stream
                .write_all(&(n as u32).to_le_bytes())
                .and_then(|_| stream.write_all(&bytes[..n]))
                .is_err()
            {
                break;
            }
        }
        let _ = stream.write_all(&0u32.to_le_bytes());
        e.store(true, Ordering::SeqCst);
    });
    admin.set_nonblocking(true)?;
    let until = Instant::now() + Duration::from_secs(300);
    while Instant::now() < until {
        match admin.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                let mut br = BufReader::new(stream);
                let mut line = String::new();
                br.by_ref().take(8193).read_line(&mut line)?;
                let v: Value = serde_json::from_str(&line)?;
                if matches!(v["action"].as_str(), Some("kill") | Some("shutdown")) {
                    let _ = child.lock().unwrap().kill();
                    let limit = Instant::now() + Duration::from_secs(3);
                    while Instant::now() < limit && child.lock().unwrap().try_wait()?.is_none() {
                        thread::sleep(Duration::from_millis(20));
                    }
                }
                let cr = credit.0.lock().unwrap();
                let exit = child.lock().unwrap().try_wait()?;
                let reply = json!({"produced":cr.produced,"acked":cr.acked,"max_inflight":cr.max,
                    "sha256":format!("{:x}",cr.digest.clone().finalize()),"ended":ended.load(Ordering::SeqCst),
                    "shell_pid":child_pid,"exit":exit.map(|x|x.exit_code())});
                writeln!(br.get_mut(), "{reply}")?;
                if v["action"] == "shutdown" {
                    std::fs::write(dir.join("server-result.json"), reply.to_string())?;
                    stopped.store(true, Ordering::SeqCst);
                    credit.1.notify_all();
                    std::process::exit(0);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(e) => return Err(e.into()),
        }
    }
    let _ = child.lock().unwrap().kill();
    std::process::exit(2);
}
