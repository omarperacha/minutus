use super::*;

#[test]
fn explicit_cancellation_lock_and_security_boundary_revoke_the_exact_attempt()
-> Result<(), Box<dyn std::error::Error>> {
    for security_event in [false, true] {
        let directory = tempfile::tempdir()?;
        let control = WalletControl::initialize(directory.path().join("synthetic-vault"), None)?;
        let job = control.begin().map_err(|error| error.message)?;
        let epoch = control.epoch.load(Ordering::Acquire);
        assert!(control.signing_attempt_is_current(&job, epoch, false));
        assert!(!control.signing_attempt_is_current(&job, epoch, true));
        assert!(!control.signing_attempt_is_current(&job, epoch.wrapping_add(1), false));
        if security_event {
            control.handle_system_sleep();
        } else {
            control.lock();
        }
        assert!(!control.signing_attempt_is_current(&job, epoch, false));
        drop(job);
        let next = control.begin().map_err(|error| error.message)?;
        assert!(!control.signing_attempt_is_current(&next, epoch, false));
    }
    Ok(())
}
