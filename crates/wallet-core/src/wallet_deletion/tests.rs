use std::time::{Duration, Instant};

use crate::{
    VaultDeletionEffects, VaultEncryptionKey, VaultInactivityTimeout, VaultKeyId, VaultLockReason,
    VaultSession, VaultSessionStatus, VaultUnlockAuthorization,
};

use super::{
    VAULT_DELETION_WARNING, VaultDeletionEffectError, VaultDeletionError, VaultDeletionPreview,
    VaultEncryptedMaterialRemoval, VaultRecoveryConfirmation, VaultRemovalStatus,
    VaultTypedConfirmation,
};

#[test]
fn preview_requires_warning_and_exact_vault_bound_text() -> Result<(), Box<dyn std::error::Error>> {
    let key_id = VaultKeyId::new("primary")?;
    let preview = VaultDeletionPreview::new(key_id.clone());
    assert_eq!(preview.warning(), VAULT_DELETION_WARNING);
    assert_eq!(preview.required_confirmation(), "DELETE primary");
    assert_eq!(
        preview
            .confirm(
                VaultRecoveryConfirmation::AllRecoveryMaterialAvailable,
                false,
                VaultTypedConfirmation::new("DELETE primary".to_owned())?,
            )
            .err(),
        Some(VaultDeletionError::DestructiveWarningNotAcknowledged)
    );

    let preview = VaultDeletionPreview::new(key_id);
    assert_eq!(
        preview
            .confirm(
                VaultRecoveryConfirmation::RecoveryUnavailableAndPermanentLossAccepted,
                true,
                VaultTypedConfirmation::new("delete primary".to_owned())?,
            )
            .err(),
        Some(VaultDeletionError::TypedConfirmationMismatch)
    );
    Ok(())
}

#[test]
fn confirmation_input_is_bounded_printable_ascii() {
    for invalid in [String::new(), "x".repeat(72), "DELETE\nprimary".to_owned()] {
        assert_eq!(
            VaultTypedConfirmation::new(invalid).err(),
            Some(VaultDeletionError::InvalidTypedConfirmation)
        );
    }
}

#[test]
fn confirmed_deletion_locks_then_removes_key_before_material()
-> Result<(), Box<dyn std::error::Error>> {
    let key_id = VaultKeyId::new("primary")?;
    let mut session = unlocked_session(key_id.clone())?;
    let commit = VaultDeletionPreview::new(key_id).confirm(
        VaultRecoveryConfirmation::AllRecoveryMaterialAvailable,
        true,
        VaultTypedConfirmation::new("DELETE primary".to_owned())?,
    )?;
    let mut effects = RecordingEffects::succeed();

    let outcome = commit.execute(&mut session, &mut effects)?;

    assert_eq!(
        session.status(),
        VaultSessionStatus::Locked(VaultLockReason::Deletion)
    );
    assert_eq!(effects.calls, ["key", "material"]);
    assert_eq!(outcome.key_status(), VaultRemovalStatus::Removed);
    assert_eq!(
        outcome.encrypted_material_status(),
        VaultRemovalStatus::Removed
    );
    assert_eq!(
        outcome.removal_guarantee(),
        VaultEncryptedMaterialRemoval::CryptographicErasureWithBestEffortPhysicalRemoval
    );
    Ok(())
}

#[test]
fn key_failure_stops_before_encrypted_material_removal() -> Result<(), Box<dyn std::error::Error>> {
    let key_id = VaultKeyId::new("primary")?;
    let mut session = unlocked_session(key_id.clone())?;
    let commit = confirmed_commit(key_id)?;
    let mut effects = RecordingEffects::fail_key();

    assert_eq!(
        commit.execute(&mut session, &mut effects),
        Err(VaultDeletionError::KeyRemovalFailed)
    );
    assert_eq!(effects.calls, ["key"]);
    assert_eq!(
        session.status(),
        VaultSessionStatus::Locked(VaultLockReason::Deletion)
    );
    Ok(())
}

#[test]
fn material_failure_reports_partial_deletion_after_key_removal()
-> Result<(), Box<dyn std::error::Error>> {
    let key_id = VaultKeyId::new("primary")?;
    let mut session = unlocked_session(key_id.clone())?;
    let commit = confirmed_commit(key_id)?;
    let mut effects = RecordingEffects::fail_material();

    assert_eq!(
        commit.execute(&mut session, &mut effects),
        Err(VaultDeletionError::EncryptedMaterialRemovalFailedAfterKeyRemoval)
    );
    assert_eq!(effects.calls, ["key", "material"]);
    Ok(())
}

#[test]
fn commit_rejects_a_session_for_another_vault() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = unlocked_session(VaultKeyId::new("other")?)?;
    let commit = confirmed_commit(VaultKeyId::new("primary")?)?;
    let mut effects = RecordingEffects::succeed();

    assert_eq!(
        commit.execute(&mut session, &mut effects),
        Err(VaultDeletionError::SessionForDifferentVault)
    );
    assert!(effects.calls.is_empty());
    assert_eq!(session.status(), VaultSessionStatus::Unlocked);
    Ok(())
}

fn confirmed_commit(key_id: VaultKeyId) -> Result<super::VaultDeletionCommit, VaultDeletionError> {
    VaultDeletionPreview::new(key_id).confirm(
        VaultRecoveryConfirmation::RecoveryUnavailableAndPermanentLossAccepted,
        true,
        VaultTypedConfirmation::new("DELETE primary".to_owned())?,
    )
}

fn unlocked_session(key_id: VaultKeyId) -> Result<VaultSession, Box<dyn std::error::Error>> {
    let mut session = VaultSession::new(
        key_id.clone(),
        VaultInactivityTimeout::new(Duration::from_secs(30))?,
    );
    session.unlock(
        VaultUnlockAuthorization::new(key_id, VaultEncryptionKey::generate()?),
        Instant::now(),
    )?;
    Ok(session)
}

struct RecordingEffects {
    calls: Vec<&'static str>,
    fail_key: bool,
    fail_material: bool,
}

impl RecordingEffects {
    const fn succeed() -> Self {
        Self {
            calls: Vec::new(),
            fail_key: false,
            fail_material: false,
        }
    }

    const fn fail_key() -> Self {
        Self {
            calls: Vec::new(),
            fail_key: true,
            fail_material: false,
        }
    }

    const fn fail_material() -> Self {
        Self {
            calls: Vec::new(),
            fail_key: false,
            fail_material: true,
        }
    }
}

impl VaultDeletionEffects for RecordingEffects {
    fn remove_vault_key(
        &mut self,
        _key_id: &VaultKeyId,
    ) -> Result<VaultRemovalStatus, VaultDeletionEffectError> {
        self.calls.push("key");
        if self.fail_key {
            Err(VaultDeletionEffectError)
        } else {
            Ok(VaultRemovalStatus::Removed)
        }
    }

    fn remove_encrypted_material(
        &mut self,
        _key_id: &VaultKeyId,
    ) -> Result<VaultRemovalStatus, VaultDeletionEffectError> {
        self.calls.push("material");
        if self.fail_material {
            Err(VaultDeletionEffectError)
        } else {
            Ok(VaultRemovalStatus::Removed)
        }
    }
}
