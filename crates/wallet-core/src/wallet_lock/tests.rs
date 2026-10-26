use std::time::{Duration, Instant};

use crate::{VaultEncryptionKey, VaultKeyId, VaultSigningAuthorization, VaultUnlockAuthorization};

use super::{
    MAX_INACTIVITY_TIMEOUT, MIN_INACTIVITY_TIMEOUT, VaultInactivityTimeout, VaultLockReason,
    VaultSession, VaultSessionError, VaultSessionStatus,
};

#[test]
fn inactivity_timeout_enforces_closed_bounds() {
    assert_eq!(
        VaultInactivityTimeout::new(Duration::ZERO),
        Err(VaultSessionError::InvalidInactivityTimeout)
    );
    assert!(VaultInactivityTimeout::new(MIN_INACTIVITY_TIMEOUT).is_ok());
    assert!(VaultInactivityTimeout::new(MAX_INACTIVITY_TIMEOUT).is_ok());
    assert_eq!(
        VaultInactivityTimeout::new(MAX_INACTIVITY_TIMEOUT + Duration::from_nanos(1)),
        Err(VaultSessionError::InvalidInactivityTimeout)
    );
}

#[test]
fn unlock_requires_matching_local_authorization() -> Result<(), Box<dyn std::error::Error>> {
    let now = Instant::now();
    let mut session = session("vault-a", Duration::from_secs(30))?;
    assert_eq!(
        session.status(),
        VaultSessionStatus::Locked(VaultLockReason::NeverUnlocked)
    );

    assert_eq!(
        session.unlock(unlock_authorization("vault-b")?, now),
        Err(VaultSessionError::AuthorizationForDifferentVault)
    );
    session.unlock(unlock_authorization("vault-a")?, now)?;
    assert_eq!(session.status(), VaultSessionStatus::Unlocked);
    assert_eq!(
        session.unlock(unlock_authorization("vault-a")?, now),
        Err(VaultSessionError::AlreadyUnlocked)
    );
    Ok(())
}

#[test]
fn manual_lock_and_system_sleep_remove_unlocked_access() -> Result<(), Box<dyn std::error::Error>> {
    let now = Instant::now();
    let mut session = session("vault-a", Duration::from_secs(30))?;
    session.unlock(unlock_authorization("vault-a")?, now)?;
    assert_eq!(session.with_unlocked_key(now, |key| key.len())?, 32);

    session.lock_manually();
    assert_eq!(
        session.status(),
        VaultSessionStatus::Locked(VaultLockReason::Manual)
    );
    assert_eq!(
        session.with_unlocked_key(now, |_| ()),
        Err(VaultSessionError::Locked)
    );

    session.unlock(unlock_authorization("vault-a")?, now)?;
    session.handle_system_sleep();
    assert_eq!(
        session.status(),
        VaultSessionStatus::Locked(VaultLockReason::SystemSleep)
    );
    Ok(())
}

#[test]
fn activity_resets_deadline_and_boundary_locks() -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let timeout = Duration::from_secs(10);
    let mut session = session("vault-a", timeout)?;
    session.unlock(unlock_authorization("vault-a")?, start)?;
    session.record_activity(start + Duration::from_secs(9))?;

    assert_eq!(
        session.enforce_inactivity(start + Duration::from_secs(18)),
        VaultSessionStatus::Unlocked
    );
    assert_eq!(
        session.enforce_inactivity(start + Duration::from_secs(19)),
        VaultSessionStatus::Locked(VaultLockReason::Inactivity)
    );
    Ok(())
}

#[test]
fn backwards_monotonic_observation_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let earlier = Instant::now();
    let start = earlier + Duration::from_nanos(1);
    let mut session = session("vault-a", Duration::from_secs(30))?;
    session.unlock(unlock_authorization("vault-a")?, start)?;

    assert_eq!(
        session.record_activity(earlier),
        Err(VaultSessionError::ClockMovedBackwards)
    );
    assert_eq!(
        session.status(),
        VaultSessionStatus::Locked(VaultLockReason::ClockDiscontinuity)
    );
    Ok(())
}

#[test]
fn signing_requires_unlocked_matching_fresh_authorization() -> Result<(), Box<dyn std::error::Error>>
{
    let now = Instant::now();
    let mut session = session("vault-a", Duration::from_secs(30))?;
    assert_eq!(
        session
            .authorize_signing(signing_authorization("vault-a")?, now)
            .err(),
        Some(VaultSessionError::Locked)
    );

    session.unlock(unlock_authorization("vault-a")?, now)?;
    assert_eq!(
        session
            .authorize_signing(signing_authorization("vault-b")?, now)
            .err(),
        Some(VaultSessionError::AuthorizationForDifferentVault)
    );
    let permit = session.authorize_signing(signing_authorization("vault-a")?, now)?;
    assert_eq!(permit.sign(|key| key.len()), 32);
    assert_eq!(session.status(), VaultSessionStatus::Unlocked);
    Ok(())
}

fn session(key_id: &str, timeout: Duration) -> Result<VaultSession, Box<dyn std::error::Error>> {
    Ok(VaultSession::new(
        VaultKeyId::new(key_id)?,
        VaultInactivityTimeout::new(timeout)?,
    ))
}

fn unlock_authorization(
    key_id: &str,
) -> Result<VaultUnlockAuthorization, Box<dyn std::error::Error>> {
    Ok(VaultUnlockAuthorization::new(
        VaultKeyId::new(key_id)?,
        VaultEncryptionKey::generate()?,
    ))
}

fn signing_authorization(
    key_id: &str,
) -> Result<VaultSigningAuthorization, Box<dyn std::error::Error>> {
    Ok(VaultSigningAuthorization::new(
        VaultKeyId::new(key_id)?,
        VaultEncryptionKey::generate()?,
    ))
}
