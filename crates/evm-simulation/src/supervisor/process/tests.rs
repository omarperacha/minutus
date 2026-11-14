use super::*;
use std::{process::Command, sync::atomic::AtomicBool};

#[test]
fn ordinary_online_transport_failures_do_not_invent_quarantine() {
    assert_eq!(
        map_remote_error(worker_transport::Error::Cancelled, false),
        SimulationError::Cancelled
    );
    assert_eq!(
        map_remote_error(worker_transport::Error::Limit, false),
        SimulationError::Limit
    );
    assert_eq!(
        map_remote_error(worker_transport::Error::UnconfirmedTermination, false),
        SimulationError::WorkerUnavailable
    );
    assert!(control::ensure_worker_runtime_available().is_ok());
}
#[test]
fn quarantine_child_probe() {
    if std::env::var_os("MINUTUS_TEST_QUARANTINE_CHILD").is_none() {
        return;
    }
    assert!(control::ensure_worker_runtime_available().is_ok());
    assert_eq!(
        map_remote_error(worker_transport::Error::UnconfirmedTermination, true),
        SimulationError::WorkerUnavailable
    );
    assert_eq!(
        control::ensure_worker_runtime_available(),
        Err(SimulationError::WorkerUnavailable)
    );
    let cancelled = AtomicBool::new(false);
    assert_eq!(
        SimulationControl::start(&cancelled).remaining(),
        Err(SimulationError::WorkerUnavailable)
    );
    assert!(crate::TrustedSimulationWorkers::packaged().is_err());
    // Subsequent ordinary errors cannot clear terminal offline quarantine.
    let _ = map_remote_error(worker_transport::Error::Cancelled, false);
    assert!(control::ensure_worker_runtime_available().is_err());
}
#[test]
fn unconfirmed_offline_reaping_is_terminal_in_a_fresh_process()
-> Result<(), Box<dyn std::error::Error>> {
    let result = Command::new(std::env::current_exe()?)
        .args([
            "supervisor::process::tests::quarantine_child_probe",
            "--exact",
        ])
        .env("MINUTUS_TEST_QUARANTINE_CHILD", "1")
        .output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}
