use super::*;
use crate::{
    RecoverySeed, VaultInactivityTimeout, VaultKeyId, VaultSession, VaultSessionStatus,
    derive_default_evm_account,
};
use std::{
    error::Error,
    time::{Duration, Instant},
};

pub(super) fn fixture(
    job: &PasswordVaultJob,
) -> Result<(WalletPassword, PasswordVaultDocument), Box<dyn Error>> {
    let password = WalletPassword::new("synthetic vault passphrase only".to_owned())?;
    let seed = RecoverySeed::new([0x42; 64]);
    let account = derive_default_evm_account(&seed)?;
    let document = PasswordVaultDocument::create_recovery(
        VaultKeyId::new("password-test")?,
        &password,
        &seed,
        vec![account.metadata("account-1", 84532)?],
        job,
    )?;
    Ok((password, document))
}

#[test]
fn wrapper_reopens_account_changes_password_and_rejects_stale_unlock() -> Result<(), Box<dyn Error>>
{
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let (password, document) = fixture(&job)?;
    let mut session = VaultSession::new(
        document.id().clone(),
        VaultInactivityTimeout::new(Duration::from_secs(30))?,
    );
    let restored = PasswordVaultDocument::from_json(&document.to_json()?)?;
    let signer = restored.authenticate_unlock(&password, &job)?.install(
        &mut session,
        Instant::now(),
        &job,
    )?;
    assert_eq!(
        signer.account().as_str(),
        document.payload().accounts()[0].address()
    );
    let wrong = WalletPassword::new("different synthetic passphrase".to_owned())?;
    assert!(matches!(
        restored.authenticate_unlock(&wrong, &job),
        Err(PasswordVaultError::AuthenticationFailed)
    ));
    let changed = restored.change_password(&password, &wrong, &job)?;
    assert_eq!(changed.payload(), restored.payload());
    assert_ne!(changed.salt, restored.salt);
    assert_ne!(changed.nonce, restored.nonce);
    assert!(matches!(
        changed.authenticate_unlock(&password, &job),
        Err(PasswordVaultError::AuthenticationFailed)
    ));
    let pending = changed.authenticate_unlock(&wrong, &job)?;
    session.lock_manually();
    work.cancel();
    assert!(matches!(
        pending.install(&mut session, Instant::now(), &job),
        Err(PasswordVaultError::Cancelled)
    ));
    assert!(matches!(session.status(), VaultSessionStatus::Locked(_)));
    Ok(())
}

#[test]
fn envelope_rejects_unreviewed_work_and_all_authenticated_field_substitutions()
-> Result<(), Box<dyn Error>> {
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let (password, document) = fixture(&job)?;
    let bytes = document.to_json()?;
    for (original, replacement) in [
        ("\"memoryKiB\":262144", "\"memoryKiB\":4294967295"),
        ("\"memoryKiB\":262144", "\"memoryKiB\":8"),
        ("\"iterations\":6", "\"iterations\":0"),
        ("\"iterations\":6", "\"iterations\":4294967295"),
        ("\"lanes\":1", "\"lanes\":2"),
        ("\"version\":19", "\"version\":16"),
        ("\"algorithm\":\"argon2id\"", "\"algorithm\":\"pbkdf2\""),
        (
            "\"algorithm\":\"xchacha20-poly1305\"",
            "\"algorithm\":\"aes\"",
        ),
    ] {
        let original_json = std::str::from_utf8(&bytes)?;
        assert!(original_json.contains(original));
        let changed = original_json.replacen(original, replacement, 1);
        assert!(PasswordVaultDocument::from_json(changed.as_bytes()).is_err());
    }
    for version in [0, 1, 3, u32::MAX] {
        assert_eq!(
            PasswordVaultDocument::from_json(format!("{{\"version\":{version}}}").as_bytes()).err(),
            Some(PasswordVaultError::UnsupportedVersion)
        );
    }
    let json = std::str::from_utf8(&bytes)?;
    for changed in [
        json.replacen("{\"version\":2,", "{\"version\":2,\"unknown\":true,", 1),
        json.replacen(
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, document.salt),
            "!",
            1,
        ),
        json.replacen(
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, document.nonce),
            "AA==",
            1,
        ),
        json.replacen(
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, document.tag),
            "AA==",
            1,
        ),
    ] {
        assert!(PasswordVaultDocument::from_json(changed.as_bytes()).is_err());
    }
    for bytes in [
        vec![b'x'; MAX_PASSWORD_VAULT_BYTES + 1],
        br#"{"version":2,"version":2}"#.to_vec(),
        b"null".to_vec(),
    ] {
        assert!(PasswordVaultDocument::from_json(&bytes).is_err());
    }
    let mut substitutions = Vec::new();
    let mut changed = document.clone();
    changed.tag[0] ^= 1;
    substitutions.push(changed);
    let mut changed = document.clone();
    changed.encrypted_key[0] ^= 1;
    substitutions.push(changed);
    let mut changed = document.clone();
    changed.nonce[0] ^= 1;
    substitutions.push(changed);
    let mut changed = document.clone();
    changed.salt[0] ^= 1;
    substitutions.push(changed);
    let mut changed = document.clone();
    changed.id = VaultKeyId::new("another-vault")?;
    substitutions.push(changed);
    for changed in substitutions {
        assert!(matches!(
            changed.authenticate_unlock(&password, &job),
            Err(PasswordVaultError::AuthenticationFailed)
        ));
    }
    // Public account and ciphertext substitution are also bound by the outer AAD.
    let seed = RecoverySeed::new([0x24; 64]);
    let key = crate::VaultEncryptionKey::generate()?;
    let account = derive_default_evm_account(&seed)?;
    let mut changed = document.clone();
    changed.payload =
        crate::seal_recovery_seed_vault(&key, &seed, vec![account.metadata("account-1", 84532)?])?;
    assert!(changed.authenticate_unlock(&password, &job).is_err());
    Ok(())
}

#[test]
fn bounded_decoder_mutation_and_truncation_smoke_never_runs_a_kdf() -> Result<(), Box<dyn Error>> {
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let (_, document) = fixture(&job)?;
    let bytes = document.to_json()?;
    for end in 0..bytes.len() {
        assert!(PasswordVaultDocument::from_json(&bytes[..end]).is_err());
    }
    for index in 0..bytes.len() {
        let mut changed = bytes.clone();
        changed[index] ^= 0x80;
        let _ = PasswordVaultDocument::from_json(&changed);
    }
    for size in [
        0,
        1,
        16,
        1024,
        MAX_PASSWORD_VAULT_BYTES,
        MAX_PASSWORD_VAULT_BYTES + 1,
    ] {
        let mut noise = vec![0; size];
        getrandom::fill(&mut noise)?;
        assert!(PasswordVaultDocument::from_json(&noise).is_err());
    }
    Ok(())
}

#[test]
fn password_signing_adapter_rejects_failed_auth_lifecycle_binding_and_consumption()
-> Result<(), Box<dyn Error>> {
    use crate::{EvmSigningKey, EvmSigningObservation};
    use transaction_policy::{ApprovalBindingInput, ApprovalCommitment, ApprovalTransactionType};
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let (password, document, mut session, key) = unlocked_fixture(&job)?;
    let binding = ApprovalBindingInput {
        signing_digest: [0x42; 32],
        chain_id: 84532,
        account: key.account().as_str(),
        session_id: "synthetic-password-negative",
        expires_at_unix: 1060,
        transaction_type: ApprovalTransactionType::Eip712,
        decoder_digest: [0x43; 32],
        simulation_digest: [0x44; 32],
    };
    let wrong = WalletPassword::new("incorrect synthetic password".to_owned())?;
    let approval = ApprovalCommitment::issue(binding, 1000)?;
    assert_eq!(
        document
            .signing_attempt(&wrong, &job)
            .sign(
                &key,
                &approval,
                &mut session,
                &FinalizationStore::default(),
                1001,
                || EvmSigningObservation::new(binding, 1002, Instant::now(), true)
            )
            .err(),
        Some(crate::EvmSigningError::AuthorizationDenied)
    );
    let approval = ApprovalCommitment::issue(binding, 1000)?;
    assert_eq!(
        document
            .signing_attempt(&password, &job)
            .sign(&key, &approval, &mut session, &Unavailable, 1001, || {
                EvmSigningObservation::new(binding, 1002, Instant::now(), true)
            })
            .err(),
        Some(crate::EvmSigningError::Approval(
            transaction_policy::ApprovalCommitmentError::ConsumptionStoreUnavailable
        ))
    );
    let approval = ApprovalCommitment::issue(binding, 1000)?;
    assert_eq!(
        document
            .signing_attempt(&password, &job)
            .sign(
                &key,
                &approval,
                &mut session,
                &FinalizationStore::default(),
                1001,
                || EvmSigningObservation::new(binding, 1002, Instant::now(), false)
            )
            .err(),
        Some(crate::EvmSigningError::ApplicationBackgrounded)
    );
    let other =
        EvmSigningKey::from_recovery_seed(document.id().clone(), &RecoverySeed::new([0x24; 64]))?;
    let approval = ApprovalCommitment::issue(binding, 1000)?;
    assert_eq!(
        document
            .signing_attempt(&password, &job)
            .sign(
                &other,
                &approval,
                &mut session,
                &FinalizationStore::default(),
                1001,
                || EvmSigningObservation::new(binding, 1002, Instant::now(), true)
            )
            .err(),
        Some(crate::EvmSigningError::ApprovalForDifferentAccount)
    );
    work.cancel();
    let approval = ApprovalCommitment::issue(binding, 1000)?;
    assert_eq!(
        document
            .signing_attempt(&password, &job)
            .sign(
                &key,
                &approval,
                &mut session,
                &FinalizationStore::default(),
                1001,
                || EvmSigningObservation::new(binding, 1002, Instant::now(), true)
            )
            .err(),
        Some(crate::EvmSigningError::AuthorizationCancelled)
    );
    Ok(())
}

fn unlocked_fixture(
    job: &PasswordVaultJob,
) -> Result<
    (
        WalletPassword,
        PasswordVaultDocument,
        VaultSession,
        crate::EvmSigningKey,
    ),
    Box<dyn Error>,
> {
    let (password, document) = fixture(job)?;
    let mut session = VaultSession::new(
        document.id().clone(),
        VaultInactivityTimeout::new(Duration::from_mins(1))?,
    );
    let key =
        document
            .authenticate_unlock(&password, job)?
            .install(&mut session, Instant::now(), job)?;
    Ok((password, document, session, key))
}

struct Unavailable;
impl transaction_policy::ApprovalConsumptionStore for Unavailable {
    fn finalize(
        &self,
        _: &transaction_policy::ApprovalConsumptionRecord<'_>,
        _: transaction_policy::ApprovalFinalization,
    ) -> transaction_policy::ApprovalConsumptionOutcome {
        transaction_policy::ApprovalConsumptionOutcome::Unavailable
    }
}
#[derive(Default)]
struct FinalizationStore(std::cell::Cell<Option<transaction_policy::ApprovalFinalization>>);
impl transaction_policy::ApprovalConsumptionStore for FinalizationStore {
    fn finalize(
        &self,
        _: &transaction_policy::ApprovalConsumptionRecord<'_>,
        value: transaction_policy::ApprovalFinalization,
    ) -> transaction_policy::ApprovalConsumptionOutcome {
        use transaction_policy::{ApprovalConsumptionOutcome as O, ApprovalFinalization as F};
        match self.0.get() {
            None => {
                self.0.set(Some(value));
                O::Finalized
            }
            Some(F::Consumed) => O::AlreadyConsumed,
            Some(F::Invalidated(_)) => O::Invalidated,
        }
    }
}
