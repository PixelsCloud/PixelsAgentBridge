//! Explicit installed-host acceptance; never opens the local OS clipboard.
//! Reuses locally stored device credentials without exporting them.
use pab_agent_core::DataPaths;
use pab_bridge::{BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, ConnectionMode, SqliteDevicePasswordProvider};
use pab_protocol::{DesktopClipboardImage, DesktopClipboardOperation as Op, RemoteDesktopInput};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, BufReader};
type Error = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), Error> {
    let (args,values)=pab_service_config::client::startup(pab_service_config::client::Role::Operator,std::env::args_os().skip(1))?;
    for (key,value) in values { unsafe { std::env::set_var(key,value); } }
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?.block_on(run(args))
}
async fn run(args:Vec<String>) -> Result<(), Error> {
    if args.len()!=2 || !["428358431", "947158375"].contains(&args[0].as_str()) || !["auto", "relay"].contains(&args[1].as_str()) {
        return Err("usage: image_clipboard_network DESIGNATED_DEVICE auto|relay".into());
    }
    let temp=tempfile::tempdir()?;
    let config=BridgeConfig::register_guest_with_secret(temp.path().join("operator.key")).await?;
    let passwords=Arc::new(SqliteDevicePasswordProvider::new(DataPaths::for_operator()?.bridge_database()));
    let mut options=BridgeRuntimeConfig::new(temp.path().join("runtime.sqlite3"));
    options.resume_incomplete_on_start=false;
    let runtime=BridgeRuntime::start(config,options,passwords).await?;
    let result=tokio::time::timeout(Duration::from_secs(600),drive(&runtime,&args[0],&args[1])).await;
    runtime.shutdown().await?;
    result??;
    Ok(())
}
async fn drive(runtime:&BridgeRuntime,code:&str,mode:&str)->Result<(),Error>{
    let device=runtime.resolve_device_code(code.parse()?).await?;
    runtime.connect_device_with_mode(device,Some(if mode=="relay"{ConnectionMode::Relay}else{ConnectionMode::Auto})).await?;
    let mut session=runtime.open_remote_desktop(device,None,15).await?;
    let first=session.next_frame(Duration::from_secs(20)).await?;
    let acknowledgements=session.presentation_control();
    // This is a protocol receiver, not a frame-rate/presentation benchmark.
    // Acknowledge consumed frames so idle command input cannot expire video.
    acknowledgements.report_presented(&first.header)?;
    let control=session.input_control();
    control.set_enabled(true).await?;
    let mut clipboard=session.clipboard_control();
    let original=clipboard.exchange(Op::Read{after:None}).await?;
    let mut owned=None;
    println!("{}",serde_json::json!({"ready":true,"clipboard":clipboard.is_supported(),"path":format!("{:?}",session.connection_path())}));
    let video=async {loop{let frame=session.next_frame_when_changed().await?;acknowledgements.report_presented(&frame.header)?;} #[allow(unreachable_code)] Ok::<(),Error>(())};
    let input=async {
        let mut lines=BufReader::new(tokio::io::stdin()).lines();
        while let Some(line)=lines.next_line().await? {
            let v:serde_json::Value=serde_json::from_str(&line).map_err(|_|"invalid test command")?;
            match v["action"].as_str().ok_or("missing action")? {
                "write"=>{
                    let data=std::fs::read(v["path"].as_str().ok_or("missing path")?)?;
                    let image=DesktopClipboardImage{width:v["width"].as_u64().ok_or("missing width")? as u32,height:v["height"].as_u64().ok_or("missing height")? as u32,bytes:data.len().try_into()?,data};
                    let expected_revision=if v["stale"].as_bool()==Some(true){Some(1)}else{None};
                    let result=clipboard.exchange(Op::WriteImage{image,expected_revision}).await;
                    if v["stale"].as_bool()==Some(true){if result.is_ok(){return Err("stale image accepted".into());}println!("{{\"stale_rejected\":true}}");}
                    else{let s=result?;owned=Some(s.revision);println!("{{\"written\":true}}");}
                }
                "read"=>{
                    let s=clipboard.exchange(Op::Read{after:None}).await?;
                    if let Some(image)=s.image {std::fs::write(v["path"].as_str().ok_or("missing path")?,image.data)?;println!("{}",serde_json::json!({"image":true,"width":image.width,"height":image.height}));}
                    else{println!("{}",serde_json::json!({"image":false,"text_len":s.text.as_ref().map(String::len)}));}
                }
                "no_echo"=>{let s=clipboard.exchange(Op::Read{after:owned}).await?;if s.changed{return Err("own write echoed".into());}println!("{{\"no_echo\":true}}");}
                "paste"=>{for (virtual_key,down) in [(0x11,true),(0x56,true),(0x56,false),(0x11,false)]{control.send(RemoteDesktopInput::Key{virtual_key,down}).await?;}println!("{{\"paste_sent\":true}}");}
                "text"=>{let s=clipboard.exchange(Op::Write{text:"Pixels image clipboard text regression".into()}).await?;owned=Some(s.revision);let s=clipboard.exchange(Op::Read{after:None}).await?;if s.text.as_deref()!=Some("Pixels image clipboard text regression")||s.image.is_some(){return Err("text regression".into());}println!("{{\"text\":true}}");}
                "close"=>break,
                _=>return Err("unknown test action".into()),
            }
        }
        Ok::<(),Error>(())
    };
    let result=tokio::select!{r=video=>r,r=input=>r};
    let current=clipboard.exchange(Op::Read{after:None}).await?;
    let restore=if owned==Some(current.revision){if let Some(image)=original.image{Some(Op::WriteImage{image,expected_revision:owned})}else{original.text.map(|text|Op::Write{text})}}else{None};
    let restored=if let Some(op)=restore{clipboard.exchange(op).await?;true}else{false};
    control.send(RemoteDesktopInput::ReleaseAll).await?;
    session.close().await?;
    println!("{}",serde_json::json!({"closed":true,"restored":restored}));
    result
}
