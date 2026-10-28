//! Opt-in two-host lab fixture, never selected by the normal unit-test adapter.
//! Transfer the public offers using Pixels, verify their hashes, and place them
//! under a fresh owned lab directory. Only the pinned ephemeral peer is served.
use super::*;
use pab_file_manager::{BatchStore,LocalFileManager,encode_path};
use pab_protocol::*;
use pab_transport::{PabEndpoint,PabEndpointAddress,PabEndpointConfig};
use serde::{Deserialize,Serialize};
use sha2::{Digest,Sha256};
use std::{net::SocketAddr,path::{Path,PathBuf}};

#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Offer { run:String,key:[u8;32],addresses:Vec<SocketAddr>,relay_urls:Vec<String>,root:NativeFilePath,path_style:PathStyle }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LabConfig {mode:pab_transport::ConnectionMode,relay_urls:Vec<String>}
fn device()->DeviceRef {DeviceRef {tenant_id:TenantId::from_u128(1),device_id:DeviceId::from_u128(2)}}
fn actor(key:[u8;32])->OperatorRef {OperatorRef::account(UserId::from_u128(3),EndpointKey::new(key))}
async fn endpoint(lab:&LabConfig)->PabEndpoint {
    let mut config=PabEndpointConfig::new(lab.relay_urls.iter().map(|url|url.parse().unwrap()).collect()).unwrap();
    if lab.mode==pab_transport::ConnectionMode::Relay {config=config.relay_only_for_testing();}
    PabEndpoint::bind(config,iroh_base::SecretKey::from_bytes(&Sha256::digest(RequestId::new().to_string().as_bytes()).into())).await.unwrap()
}
async fn write_offer(base:&Path,run:&str,name:&str,endpoint:&PabEndpoint,root:&Path,lab:&LabConfig) {
    let mut addresses=endpoint.watch_address();
    tokio::time::timeout(Duration::from_secs(30),async {while if lab.mode==pab_transport::ConnectionMode::Relay {addresses.borrow().relay_urls.is_empty()}else{addresses.borrow().direct_addresses.is_empty()}{addresses.changed().await.unwrap();}}).await.unwrap();
    let offer=Offer {run:run.into(),key:*endpoint.id().as_bytes(),addresses:addresses.borrow().direct_addresses.clone(),relay_urls:addresses.borrow().relay_urls.clone(),root:encode_path(root),path_style:pab_file_manager::locations().unwrap().path_style};
    let bytes=serde_json::to_vec_pretty(&offer).unwrap();
    let path=base.join(name);let mut output=std::fs::OpenOptions::new().create_new(true).write(true).open(&path).unwrap();
    use std::io::Write;output.write_all(&bytes).unwrap();output.sync_all().unwrap();
    println!("OFFER {} sha256={:x}",path.display(),Sha256::digest(&bytes));
}
async fn read_offer(base:&Path,name:&str,run:&str)->Offer {
    tokio::time::timeout(Duration::from_secs(240),async {loop {
        let path=base.join(name);
        if let Ok(bytes)=std::fs::read(&path) {assert!(bytes.len()<16*1024);let offer:Offer=serde_json::from_slice(&bytes).unwrap();assert_eq!(offer.run,run);assert!(!offer.addresses.is_empty() || !offer.relay_urls.is_empty());assert!(offer.addresses.len()<=64 && offer.relay_urls.len()<=8);return offer;}
        tokio::time::sleep(Duration::from_millis(200)).await;
    }}).await.expect("the verified peer offer was not supplied via Pixels")
}

#[tokio::test]
#[ignore="requires explicit provisioned two-host Pixels lab and exchanged public offers"]
async fn two_host_native_batch_fixture() {Box::pin(lab()).await;}
async fn lab() {
    let base=PathBuf::from(std::env::var_os("PAB_FT_LAB_ROOT").expect("PAB_FT_LAB_ROOT"));
    assert!(base.is_absolute());let base=base.canonicalize().unwrap();
    let run=std::env::var("PAB_FT_LAB_RUN").expect("PAB_FT_LAB_RUN");let _:RequestId=run.parse().expect("fresh run UUID");
    assert_eq!(base.file_name().unwrap(),format!("network-{run}").as_str(),"use a fresh lab directory, never application data");
    let role=std::env::var("PAB_FT_LAB_ROLE").expect("PAB_FT_LAB_ROLE");assert!(matches!(role.as_str(),"server"|"client"));
    assert_eq!(std::fs::read_to_string(base.join("owned-run.txt")).unwrap(),run);
    let temporary=tempfile::Builder::new().prefix("native-batch-").tempdir_in(&base).unwrap();let root=temporary.path().canonicalize().unwrap();
    let lab:LabConfig=pab_service_config::load(&base.join("lab.toml")).unwrap();
    let endpoint=endpoint(&lab).await;
    write_offer(&base,&run,&format!("{role}-offer.json"),&endpoint,&root,&lab).await;
    if role=="server" {serve(&base,&run,&root,&endpoint).await;}else{client(&base,&run,&root,&endpoint,&lab).await;}
    endpoint.close().await;
    println!("LAB {role} completed; fixture root {}",root.display());
    super::file_fixture_cleanup::cleanup_fixture(temporary).await.unwrap();
}
async fn serve(base:&Path,run:&str,root:&Path,endpoint:&PabEndpoint) {
    let peer=read_offer(base,"client-offer.json",run).await;
    let service=pab_executor::test_support::ProtocolService::open(&root.join("executor.sqlite3"),device(),pab_platform::detect_native_execution_context().unwrap()).await.unwrap();
    let connection=tokio::time::timeout(Duration::from_secs(60),async {loop {
        let connection=endpoint.accept().await.unwrap().unwrap();
        if connection.remote_endpoint_key()==peer.key {return connection;}connection.close(b"not the pinned lab peer");
    }}).await.expect("pinned lab peer did not connect");
    let mut tasks=tokio::task::JoinSet::new();let deadline=tokio::time::sleep(Duration::from_secs(300));tokio::pin!(deadline);
    loop {tokio::select! {
        _=&mut deadline=>panic!("lab session deadline exceeded"),
        _=connection.closed()=>break,
        stream=connection.accept_bi(Duration::from_secs(30))=>{let Ok(stream)=stream else {if connection.is_closed(){break;}continue;};let service=service.clone();let actor=actor(peer.key);tasks.spawn(async move {Box::pin(service.handle_stream(actor,stream,Duration::from_secs(30))).await});},
        Some(result)=tasks.join_next(),if !tasks.is_empty()=>{result.unwrap().unwrap();},
    }}
    while let Some(result)=tasks.join_next().await {result.unwrap().unwrap();}
}
async fn client(base:&Path,run:&str,root:&Path,endpoint:&PabEndpoint,lab:&LabConfig) {
    let peer=read_offer(base,"server-offer.json",run).await;
    let connection=endpoint.connect_with_mode(peer.key,&PabEndpointAddress {relay_urls:peer.relay_urls,direct_addresses:peer.addresses},Duration::from_secs(30),lab.mode).await.unwrap();
    assert_eq!(connection.remote_endpoint_key(),peer.key);
    println!("PATH {:?} RTT {:?}",connection.selected_path(),connection.selected_path_rtt());
    if lab.mode==pab_transport::ConnectionMode::Relay {assert_eq!(connection.selected_path(),Some(pab_transport::ConnectionPath::Relay));}
    let remote=peer.root;
    let authenticated=crate::AuthenticatedDeviceConnection::file_lab_fixture(connection,device(),actor(*endpoint.id().as_bytes()));
    let (mut runtime,_credentials)=test_support::runtime(root).await;let origin="https://two-host-file-fixture.invalid";
    Arc::get_mut(&mut Arc::get_mut(&mut runtime).unwrap().inner).unwrap().store.account_origin=Some(origin.into());
    sqlx::query("INSERT INTO runtime_session_servers(session_id,origin) VALUES('fixture',?)").bind(origin).execute(&runtime.inner.store.pool).await.unwrap();
    let force_relay=lab.mode==pab_transport::ConnectionMode::Relay;
    runtime.inner.device_codes.lock().await.insert(device(),"123456789".parse().unwrap());runtime.inner.file_device(device(),force_relay).await.unwrap().set_test_connection(authenticated).await;
    let owner=runtime.file_operation_owner().await.unwrap();let store=BatchStore::open(&root.join("batches.sqlite3")).await.unwrap();let local=Arc::new(LocalFileManager::open(&root.join("local.sqlite3")).await.unwrap());
    let source=root.join("source");let received=root.join("received");std::fs::create_dir_all(source.join("nested/empty")).unwrap();std::fs::create_dir(&received).unwrap();
    let mut payload=std::fs::File::create(source.join("payload.bin")).unwrap();use std::io::Write;
    let chunk:Vec<u8>=(0..65_536).map(|n|(n%251)as u8).collect();for _ in 0..256 {payload.write_all(&chunk).unwrap();}payload.sync_all().unwrap();drop(payload);
    std::fs::write(source.join("nested/中文.bin"),b"native two-host file\0").unwrap();std::fs::write(source.join("zero"),[]).unwrap();
    let batch=|direction,sources,destination|FileBatchSpec {id:RequestId::new(),device:device(),code:"123456789".parse().unwrap(),direction,sources,destination,execution:ExecutionSelection::Service {},conflict:FileConflictPolicy::Ask,force_relay};
    let started=std::time::Instant::now();
    let (upload,_)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),batch(FileTransferDirection::Upload,vec![encode_path(&source)],remote.clone()),owner.clone()).await.unwrap();upload.launch();wait(&upload).await;
    let upload_ms=started.elapsed().as_millis();let remote_source=pab_file_manager::native_join(&remote,&"source".into(),peer.path_style).unwrap();
    let started=std::time::Instant::now();let (download,_)=ManagedBatchJob::prepare(runtime.clone(),store,local,batch(FileTransferDirection::Download,vec![remote_source],encode_path(&received)),owner).await.unwrap();download.launch();wait(&download).await;
    for path in ["payload.bin","zero","nested/中文.bin"] {assert_eq!(std::fs::read(source.join(path)).unwrap(),std::fs::read(received.join("source").join(path)).unwrap());}
    assert!(received.join("source/nested/empty").is_dir());
    println!("RESULT files=3 payload_bytes=16777216 upload_ms={upload_ms} download_ms={} hash_match=true empty_directory=true",started.elapsed().as_millis());
    while upload.running() || download.running(){tokio::task::yield_now().await;}drop((upload,download));
    tokio::time::timeout(Duration::from_secs(5),async {while Arc::strong_count(&runtime)!=1{tokio::task::yield_now().await;}}).await.unwrap();
    runtime.inner.file_devices.lock().await.clear();Arc::try_unwrap(runtime).ok().unwrap().shutdown().await.unwrap();
}
async fn wait(job:&Arc<ManagedBatchJob>) {
    tokio::time::timeout(Duration::from_secs(120),async {loop {let snapshot=job.snapshot().await.unwrap();if snapshot.phase=="completed"{assert_eq!(snapshot.completed.0,6);break;}assert!(!matches!(snapshot.phase.as_str(),"failed"|"partial_failed"|"waiting_conflict"),"batch failed: {snapshot:?}; {:?}",job.items(0).await);tokio::time::sleep(Duration::from_millis(100)).await;}}).await.expect("two-host transfer deadline exceeded");
}
