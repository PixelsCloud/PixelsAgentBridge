use crate::StagingTarget;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    io::{self, BufRead, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn child(root: &Path, mode: &str) -> (ChildGuard, mpsc::Receiver<String>) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.args([
            "--exact",
            "target_reservation_tests::reservation_child",
            "--ignored",
            "--nocapture",
        ])
        .env("PAB_TARGET_RESERVATION_TEST_ROOT", root)
        .env("PAB_TARGET_RESERVATION_TEST_MODE", mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    (ChildGuard(child), receive)
}
fn ready(receive: &mpsc::Receiver<String>) {
    loop {
        if receive
            .recv_timeout(Duration::from_secs(15))
            .expect("child reservation acknowledgement")
            .contains("RESERVATION_READY")
        {
            break;
        }
    }
}

#[test]
#[ignore = "child process helper, invoked by cross-process reservation tests"]
fn reservation_child() {
    let root = std::path::PathBuf::from(
        std::env::var_os("PAB_TARGET_RESERVATION_TEST_ROOT").expect("owned fixture required"),
    );
    assert!(root.is_absolute());
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("pixels-target-reservation-")
    );
    assert_eq!(
        std::fs::read(root.join("fixture-marker")).unwrap(),
        b"pixels-owned-target-reservation-fixture"
    );
    let mode = std::env::var("PAB_TARGET_RESERVATION_TEST_MODE").unwrap();
    let target = StagingTarget::new(&root.join(".child-attempt"), &root.join("RÉSUMÉ.BIN"));
    if mode == "contend" {
        assert!(matches!(target, Err(error) if error.kind() == io::ErrorKind::WouldBlock));
        assert!(!root.join(".child-attempt").exists());
        return;
    }
    assert_eq!(mode, "hold");
    let target = target.unwrap();
    let mut file = target.open(true).unwrap();
    file.write_all(b"child partial").unwrap();
    println!("RESERVATION_READY");
    io::stdout().flush().unwrap();
    let mut input = [0];
    let _ = io::stdin().read(&mut input).unwrap();
    drop(file);
    drop(target);
}

#[test]
fn different_process_attempts_share_the_target_and_release_it_on_process_death() {
    let mut builder = tempfile::Builder::new();
    builder.prefix("pixels-target-reservation-");
    let temp = if let Some(parent) = std::env::var_os("PAB_FILE_TRANSFER_FIXTURE_ROOT") {
        let parent = std::path::PathBuf::from(parent);
        assert!(parent.is_absolute());
        let directory = crate::mutation::open_directory(&parent).unwrap();
        drop(directory);
        builder.tempdir_in(parent).unwrap()
    } else {
        builder.tempdir().unwrap()
    };
    let root = temp.path().canonicalize().unwrap();
    eprintln!("PAB_FILE_TRANSFER_FIXTURE_ROOT={}", root.display());
    std::fs::write(
        root.join("fixture-marker"),
        b"pixels-owned-target-reservation-fixture",
    )
    .unwrap();
    #[cfg(windows)]
    let destination = root.join("Résumé.bin");
    #[cfg(target_os="linux")]
    let destination = root.join("RÉSUMÉ.BIN");
    let first = StagingTarget::new(&root.join(".first-attempt"), &destination).unwrap();
    let writer = first.open(true).unwrap();
    let (mut contender, output) = child(&root, "contend");
    // Keep stdout drained and bounded by process completion instead of waiting
    // for the holder's ready marker in this negative child.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = contender.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "contender did not terminate"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(output);
    drop(writer);
    drop(first);
    let (mut holder, output) = child(&root, "hold");
    ready(&output);
    assert!(
        matches!(StagingTarget::new(&root.join(".later-attempt"), &destination), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    assert!(StagingTarget::new(&root.join(".independent"), &root.join("other.bin")).is_ok());
    holder.0.kill().unwrap();
    holder.0.wait().unwrap();
    let recovered = StagingTarget::new(&root.join(".recovered"), &destination).unwrap();
    recovered.open(true).unwrap();
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read(root.join(".child-attempt")).unwrap(),
        b"child partial"
    );
    // Kernel reservations leave no lock files or lock databases behind.
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 4);
}

#[test]
fn physical_target_aliases_are_reserved_until_publication_owner_releases_them() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let destination = root.join("long-original-name.bin");
    let alias = root.join("alias.bin");
    std::fs::write(&destination, b"old").unwrap();
    std::fs::hard_link(&destination, &alias).unwrap();
    let first = StagingTarget::new(&root.join(".first"), &destination).unwrap();
    assert!(
        matches!(StagingTarget::new(&root.join(".alias"), &alias), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    let mut writer = first.open(true).unwrap();
    writer.write_all(b"new").unwrap();
    first.publish_open(&writer, true).unwrap();
    drop(writer);
    let published_alias = root.join("published-alias.bin");
    std::fs::hard_link(&destination, &published_alias).unwrap();
    assert!(StagingTarget::new(&root.join(".other"), &published_alias).is_err());
    drop(first);
    assert!(StagingTarget::new(&root.join(".other"), &published_alias).is_ok());
    assert_eq!(std::fs::read(destination).unwrap(), b"new");
    assert_eq!(std::fs::read(alias).unwrap(), b"old");
}

#[cfg(target_os="linux")]
#[test]
fn case_sensitive_and_non_utf8_siblings_have_independent_reservations() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let lower = StagingTarget::new(&root.join(".lower"), &root.join("resume.bin")).unwrap();
    let upper = StagingTarget::new(&root.join(".upper"), &root.join("RESUME.BIN")).unwrap();
    let native_name = OsString::from_vec(b"resume-\xff.bin".to_vec());
    let native = StagingTarget::new(&root.join(".native"), &root.join(&native_name)).unwrap();
    assert!(matches!(
        StagingTarget::new(&root.join(".duplicate"), &root.join(native_name)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock
    ));
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    drop((lower, upper, native));
}
