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
fn transport_cohort_must_match_both_original_worker_pins() -> Result<(), Box<dyn std::error::Error>>
{
    let worker = TrustedSimulationWorker {
        path: PathBuf::from("/unused"),
        sha256: [0x11; 32],
        cdhash: [0x22; 20],
        role: Role::Offline,
    };
    worker.check_cohort(&"11".repeat(32), &"22".repeat(20))?;
    assert!(
        worker
            .check_cohort(&"33".repeat(32), &"22".repeat(20))
            .is_err()
    );
    assert!(
        worker
            .check_cohort(&"11".repeat(32), &"33".repeat(20))
            .is_err()
    );
    assert!(
        worker
            .check_cohort(&"11".repeat(32), &"2A".repeat(20))
            .is_err()
    );
    Ok(())
}
