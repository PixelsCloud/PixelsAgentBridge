use super::*;
use std::os::unix::fs::PermissionsExt;
use std::{io::{BufRead, Read, Write},process::{Child,Command,Stdio},sync::mpsc,time::Duration};

fn fixture()->(tempfile::TempDir,std::path::PathBuf) {
    let temp=tempfile::Builder::new().prefix("pixels-private-staging-").tempdir().unwrap();
    let root=temp.path().canonicalize().unwrap();
    std::fs::write(root.join("fixture-marker"),b"pixels-owned-private-file-staging").unwrap();
    (temp,root)
}
fn write_receipt(root:&Path,receipt:&PrivateFileStagingReceipt) {
    let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(root.join("receipt.json")).unwrap();
    file.write_all(&serde_json::to_vec(receipt).unwrap()).unwrap();file.sync_all().unwrap();
}
struct ChildGuard(Child);
impl Drop for ChildGuard {fn drop(&mut self){let _=self.0.kill();let _=self.0.wait();}}

#[test]
#[ignore="independent writer helper invoked by process-death recovery test"]
fn staging_writer_child() {
    let root=std::path::PathBuf::from(std::env::var_os("PAB_PRIVATE_STAGING_TEST_ROOT").expect("owned fixture required"));
    assert!(root.is_absolute());
    assert!(root.file_name().unwrap().to_str().unwrap().starts_with("pixels-private-staging-"));
    assert_eq!(std::fs::read(root.join("fixture-marker")).unwrap(),b"pixels-owned-private-file-staging");
    let (stage,mut file)=PrivateFileStaging::create(&root.join("destination"),true,None).unwrap();
    write_receipt(&root,stage.receipt());
    file.write_all(b"durable partial before forced process death").unwrap();file.sync_all().unwrap();
    println!("PRIVATE_STAGING_READY");std::io::stdout().flush().unwrap();
    let mut byte=[0];let _=std::io::stdin().read(&mut byte).unwrap();
    drop((file,stage));
}

#[test]
fn process_death_reopens_only_recorded_payload_and_discards_it_idempotently() {
    let (_temp,root)=fixture();
    std::fs::write(root.join("destination"),b"original destination").unwrap();
    let mut child=Command::new(std::env::current_exe().unwrap())
        .args(["--exact","private_staging::tests::staging_writer_child","--ignored","--nocapture"])
        .env("PAB_PRIVATE_STAGING_TEST_ROOT",&root)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let stdout=child.stdout.take().unwrap();let mut child=ChildGuard(child);
    let (send,receive)=mpsc::channel();
    std::thread::spawn(move || {for line in io::BufReader::new(stdout).lines(){if send.send(line.unwrap()).is_err(){break;}}});
    loop {if receive.recv_timeout(Duration::from_secs(15)).expect("private writer readiness").contains("PRIVATE_STAGING_READY"){break;}}
    let receipt:PrivateFileStagingReceipt=serde_json::from_slice(&std::fs::read(root.join("receipt.json")).unwrap()).unwrap();
    assert!(PrivateFileStaging::reopen(&receipt).is_err(),"a live writer must retain exclusive ownership");
    child.0.kill().unwrap();assert!(!child.0.wait().unwrap().success());
    let (stage,mut file)=PrivateFileStaging::reopen(&receipt).unwrap();
    let mut bytes=Vec::new();file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes,b"durable partial before forced process death");
    assert_eq!(std::fs::metadata(root.join(&receipt.directory_name)).unwrap().mode()&0o777,0o700);
    assert_eq!(file.metadata().unwrap().mode()&0o777,0o600);
    stage.discard_open(&file).unwrap();stage.discard_open(&file).unwrap();
    assert_eq!(file.metadata().unwrap().nlink(),0);
    assert!(!root.join(&receipt.directory_name).join("payload").exists());
    assert!(root.join(&receipt.directory_name).is_dir(),"empty container remains a separately tracked resource");
    assert_eq!(std::fs::read(root.join("destination")).unwrap(),b"original destination");
    assert!(stage.retire_empty_container().unwrap());assert!(stage.retire_empty_container().unwrap());
    assert!(!root.join(&receipt.directory_name).exists());
    drop((file,stage));
    assert!(PrivateFileStaging::reopen(&receipt).is_err(),"absence is not an invented cleanup acknowledgement");
}

#[test]
fn retained_private_directory_is_not_redirected_by_a_public_name_replacement() {
    let (_temp,root)=fixture();let (stage,mut file)=PrivateFileStaging::create(&root.join("destination"),false,None).unwrap();
    file.write_all(b"owned payload").unwrap();file.sync_all().unwrap();let receipt=stage.receipt().clone();
    let original=root.join(&receipt.directory_name);let moved=root.join("moved-private-container");
    std::fs::rename(&original,&moved).unwrap();std::fs::create_dir(&original).unwrap();
    std::fs::set_permissions(&original,std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(original.join("payload"),b"keep replacement payload").unwrap();
    stage.discard_open(&file).unwrap();assert!(!moved.join("payload").exists());
    assert_eq!(std::fs::read(original.join("payload")).unwrap(),b"keep replacement payload");
    drop((stage,file));
    assert!(PrivateFileStaging::reopen(&receipt).is_err());
    assert_eq!(std::fs::read(original.join("payload")).unwrap(),b"keep replacement payload");
}

#[test]
fn cleanup_rejects_foreign_handles_changed_objects_and_nonprivate_containers() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp,root)=fixture();let (stage,mut file)=PrivateFileStaging::create(&root.join("destination"),false,None).unwrap();
    file.write_all(b"owned payload").unwrap();file.sync_all().unwrap();
    let private=root.join(&stage.receipt().directory_name);let other=root.join("foreign");
    std::fs::write(&other,b"keep foreign").unwrap();let foreign=File::open(&other).unwrap();
    assert!(stage.discard_open(&foreign).is_err());assert_eq!(std::fs::read(&other).unwrap(),b"keep foreign");
    std::fs::set_permissions(&private,std::fs::Permissions::from_mode(0o750)).unwrap();
    assert!(stage.discard_open(&file).is_err());assert!(private.join("payload").exists());
    std::fs::set_permissions(&private,std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::rename(private.join("payload"),private.join("original")).unwrap();
    std::fs::write(private.join("payload"),b"keep replacement").unwrap();
    assert!(stage.discard_open(&file).is_err());
    assert_eq!(std::fs::read(private.join("payload")).unwrap(),b"keep replacement");
    assert_eq!(std::fs::read(private.join("original")).unwrap(),b"owned payload");
}

#[test]
fn receipt_validation_never_creates_truncates_or_adopts_unrecorded_data() {
    let (_temp,root)=fixture();let (stage,mut file)=PrivateFileStaging::create(&root.join("destination"),false,None).unwrap();
    file.write_all(b"original partial").unwrap();file.sync_all().unwrap();let receipt=stage.receipt().clone();drop((file,stage));
    for field in 0..5 {
        let mut forged=receipt.clone();
        match field {0=>forged.version=2,1=>forged.directory_name="../foreign".into(),2=>forged.parent_identity="u:0:0:unknown".into(),3=>forged.directory_identity="u:0:0:unknown".into(),_=>forged.payload_identity="u:0:0:unknown".into()}
        assert!(PrivateFileStaging::reopen(&forged).is_err());
        assert_eq!(std::fs::read(root.join(&receipt.directory_name).join("payload")).unwrap(),b"original partial");
    }
    let missing=root.join("missing-parent/destination");assert!(PrivateFileStaging::create(&missing,false,None).is_err());
    assert!(!root.join("missing-parent").exists());
    let (stage,file)=PrivateFileStaging::reopen(&receipt).unwrap();stage.discard_open(&file).unwrap();
}

#[test]
fn atomic_publication_uses_private_payload_and_protects_late_conflicts() {
    let (_temp,root)=fixture();let destination=root.join("destination");
    let (stage,mut file)=PrivateFileStaging::create(&destination,false,None).unwrap();
    file.write_all(b"complete verified payload").unwrap();file.sync_all().unwrap();
    std::fs::write(&destination,b"late conflict").unwrap();
    assert_eq!(stage.publish_open(&file,false).unwrap_err().kind(),io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&destination).unwrap(),b"late conflict");
    assert!(root.join(&stage.receipt().directory_name).join("payload").exists());
    let publication=stage.publish_open(&file,true).unwrap();assert!(publication.published);assert!(!publication.staging_retained);
    assert!(!root.join(&stage.receipt().directory_name).exists());
    assert!(stage.discard_open(&file).is_err(),"a published object cannot be discarded as a partial");
    assert_eq!(std::fs::read(&destination).unwrap(),b"complete verified payload");
}

#[test]
fn container_retirement_preserves_unrecorded_children_and_unprotected_parent_entries() {
    let (_temp,root)=fixture();let (stage,file)=PrivateFileStaging::create(&root.join("destination"),false,None).unwrap();
    let container=root.join(&stage.receipt().directory_name);
    assert!(stage.retire_empty_container().is_err(),"live payload must keep its container");
    stage.discard_open(&file).unwrap();
    std::fs::write(container.join("unrecorded"),b"keep unrelated child").unwrap();
    assert!(stage.retire_empty_container().is_err());assert_eq!(std::fs::read(container.join("unrecorded")).unwrap(),b"keep unrelated child");
    std::fs::remove_file(container.join("unrecorded")).unwrap();
    std::fs::set_permissions(&root,std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(!stage.retire_empty_container().unwrap());assert!(container.is_dir());
    std::fs::set_permissions(&root,std::fs::Permissions::from_mode(0o1777)).unwrap();
    assert!(stage.retire_empty_container().unwrap());
    std::fs::set_permissions(&root,std::fs::Permissions::from_mode(0o700)).unwrap();
}
