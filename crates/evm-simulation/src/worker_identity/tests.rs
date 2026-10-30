use super::*;
#[test]
fn absent_host_manifest_never_trusts_diagnostic_test_binary() {
    assert!(TrustedSimulationWorkers::packaged().is_err());
}
#[test]
fn invalid_pins_names_and_extra_fields_fail() {
    let valid = format!(
        "minutus-evm-workers-v1\nminutus-evm-simulator {} {}\nminutus-base-offline-simulator {} {}\n",
        "00".repeat(32),
        "11".repeat(20),
        "22".repeat(32),
        "33".repeat(20)
    );
    assert!(parse(valid.as_bytes(), Path::new("/tmp")).is_ok());
    for bad in [
        valid.replace("minutus-evm-workers-v1", "v2"),
        valid.replace("minutus-evm-simulator", "../evil"),
        format!("{valid}extra\n"),
        valid.replace(&"11".repeat(20), "00"),
        valid.replace(&"22".repeat(32), &"FF".repeat(32)),
    ] {
        assert!(parse(bad.as_bytes(), Path::new("/tmp")).is_err());
    }
}
#[test]
fn changed_file_and_symlink_cannot_pass_expected_digest() -> Result<(), Box<dyn std::error::Error>>
{
    use std::io::Write;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("worker");
    let mut file = File::create(&path)?;
    file.write_all(b"reviewed")?;
    drop(file);
    let mut file = File::open(&path)?;
    let worker = TrustedSimulationWorker {
        path: path.clone(),
        sha256: digest(&mut file)?,
        cdhash: [0; 20],
        role: Role::Online,
    };
    worker.verify_file()?;
    std::fs::write(&path, b"replaced")?;
    assert!(worker.verify_file().is_err());
    #[cfg(unix)]
    {
        std::fs::remove_file(&path)?;
        std::os::unix::fs::symlink("elsewhere", &path)?;
        assert!(worker.verify_file().is_err());
    }
    assert!(worker.require(Role::Offline).is_err());
    Ok(())
}
#[test]
#[cfg(target_os = "macos")]
fn kernel_running_image_hash_is_checked_instead_of_path() -> Result<(), Box<dyn std::error::Error>>
{
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let child = Child(
        std::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()?,
    );
    let cdhash = crate::native_identity::child_hash(child.0.id())?;
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let mut worker = TrustedSimulationWorker {
        path: PathBuf::from("/unrelated/path"),
        sha256: [0; 32],
        cdhash,
        role: Role::Online,
    };
    worker.await_running(child.0.id(), &control)?;
    worker.cdhash = [0; 20];
    assert!(worker.await_running(child.0.id(), &control).is_err());
    Ok(())
}
#[test]
#[cfg(target_os = "macos")]
fn early_exit_is_rejected_without_reaping_before_attestation()
-> Result<(), Box<dyn std::error::Error>> {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(std::process::Command::new("/usr/bin/true").spawn()?);
    for _ in 0..100 {
        if crate::native_identity::child_exited(child.0.id())? {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(crate::native_identity::child_exited(child.0.id())?);
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let worker = TrustedSimulationWorker {
        path: PathBuf::from("/usr/bin/true"),
        sha256: [0; 32],
        cdhash: [0; 20],
        role: Role::Online,
    };
    assert!(
        worker
            .await_running(child.0.id(), &SimulationControl::start(&cancelled))
            .is_err()
    );
    // WNOWAIT left the exit record owned and available for our normal reap.
    assert!(child.0.wait()?.success());
    Ok(())
}
