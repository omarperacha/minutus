use std::{cell::Cell, error::Error, fmt::Write as _, time::Duration};

use bip32::secp256k1::ecdsa::{RecoveryId, Signature, VerifyingKey};
use transaction_policy::{
    ApprovalCommitment, ApprovalConsumptionOutcome, ApprovalConsumptionRecord,
    ApprovalConsumptionStore, ApprovalFinalization, ApprovalTransactionType,
};

use super::*;
use crate::{
    EvmPrivateKeyImportPreview, EvmPrivateKeyInput, VaultEncryptionKey, VaultInactivityTimeout,
    VaultUnlockAuthorization,
};

const NOW: u64 = 1_800_000_000;
const PRIVATE_KEY_ONE: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const ACCOUNT_ONE: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";

struct FakeAuthorizer {
    result: Result<(), VaultKeyStoreError>,
    calls: Cell<usize>,
}

#[derive(Default)]
struct FakeConsumptionStore {
    finalization: Cell<Option<ApprovalFinalization>>,
}

impl ApprovalConsumptionStore for FakeConsumptionStore {
    fn finalize(
        &self,
        _record: &ApprovalConsumptionRecord<'_>,
        finalization: ApprovalFinalization,
    ) -> ApprovalConsumptionOutcome {
        match self.finalization.get() {
            None => {
                self.finalization.set(Some(finalization));
                ApprovalConsumptionOutcome::Finalized
            }
            Some(ApprovalFinalization::Consumed) => ApprovalConsumptionOutcome::AlreadyConsumed,
            Some(ApprovalFinalization::Invalidated(_)) => ApprovalConsumptionOutcome::Invalidated,
        }
    }
}

impl FakeAuthorizer {
    fn success() -> Self {
        Self {
            result: Ok(()),
            calls: Cell::new(0),
        }
    }

    fn failing(error: VaultKeyStoreError) -> Self {
        Self {
            result: Err(error),
            calls: Cell::new(0),
        }
    }
}

impl SigningAuthorizer for FakeAuthorizer {
    fn authorize(
        &self,
        key_id: &VaultKeyId,
    ) -> Result<VaultSigningAuthorization, VaultKeyStoreError> {
        self.calls.set(self.calls.get() + 1);
        self.result?;
        Ok(VaultSigningAuthorization::new(
            key_id.clone(),
            VaultEncryptionKey::generate()?,
        ))
    }
}

#[test]
fn exact_approval_and_fresh_authorization_produce_recoverable_secp256k1_signature()
-> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let key = signing_key("vault-a")?;
    let mut session = unlocked_session("vault-a", start)?;
    let input = binding();
    let commitment = ApprovalCommitment::issue(input, NOW)?;
    let authorizer = FakeAuthorizer::success();
    let consumption = FakeConsumptionStore::default();

    let signature = authorize_and_sign(
        &key,
        &commitment,
        &mut session,
        &authorizer,
        &consumption,
        NOW,
        || observation(input, NOW + 1, start + Duration::from_secs(1), true),
    )?;

    assert_eq!(authorizer.calls.get(), 1);
    assert_eq!(
        hex(signature.r()),
        "e7c93726a865578504442b1a6827f676e0ed74bdff2be3960d1e253bbcfc4462"
    );
    assert_eq!(
        hex(signature.s()),
        "6aa772b878bc912bdbb33a0014ec507c4b3896ea85aa914b74dee9b7ac3e56da"
    );
    assert!(signature.y_parity());
    let signature_bytes = [signature.r().as_slice(), signature.s().as_slice()].concat();
    let signature_value =
        Signature::from_slice(&signature_bytes).map_err(|_| EvmSigningError::SignatureFailed)?;
    let recovery_id = RecoveryId::new(signature.y_parity(), false);
    let recovered =
        VerifyingKey::recover_from_prehash(&input.signing_digest, &signature_value, recovery_id)
            .map_err(|_| EvmSigningError::SignatureFailed)?;
    let mut private_key_one = [0_u8; 32];
    private_key_one[31] = 1;
    let expected =
        SigningKey::from_slice(&private_key_one).map_err(|_| EvmSigningError::InvalidPrivateKey)?;
    assert_eq!(recovered, *expected.verifying_key());
    assert_eq!(signature_value.normalize_s(), signature_value);
    // The new RustCrypto API returns a normalised signature rather than Option.
    // Preserve the host's rejection of the equivalent noncanonical high-s form.
    let high_s = -signature_value.s();
    let noncanonical = EvmTransactionSignature {
        r: *signature.r(),
        s: high_s.to_bytes().into(),
        y_parity: !signature.y_parity(),
    };
    assert_eq!(
        noncanonical.verify(&input.signing_digest, ACCOUNT_ONE),
        Err(EvmSigningError::SignatureFailed)
    );
    Ok(())
}

#[test]
fn exact_eip712_approval_uses_the_same_single_use_user_presence_gate() -> Result<(), Box<dyn Error>>
{
    let start = Instant::now();
    let key = signing_key("vault-a")?;
    let mut session = unlocked_session("vault-a", start)?;
    let mut input = binding();
    input.transaction_type = ApprovalTransactionType::Eip712;
    let commitment = ApprovalCommitment::issue(input, NOW)?;
    let authorizer = FakeAuthorizer::success();
    let consumption = FakeConsumptionStore::default();

    let signature = authorize_and_sign(
        &key,
        &commitment,
        &mut session,
        &authorizer,
        &consumption,
        NOW,
        || observation(input, NOW + 1, start + Duration::from_secs(1), true),
    )?;
    assert_ne!(signature.r(), &[0; 32]);
    assert_eq!(
        consumption.finalization.get(),
        Some(ApprovalFinalization::Consumed)
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[test]
fn duplicate_signing_attempt_is_rejected_after_fresh_authorization() -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let key = signing_key("vault-a")?;
    let mut session = unlocked_session("vault-a", start)?;
    let input = binding();
    let commitment = ApprovalCommitment::issue(input, NOW)?;
    let authorizer = FakeAuthorizer::success();
    let consumption = FakeConsumptionStore::default();

    let _signature = authorize_and_sign(
        &key,
        &commitment,
        &mut session,
        &authorizer,
        &consumption,
        NOW,
        || observation(input, NOW + 1, start + Duration::from_secs(1), true),
    )?;
    assert_eq!(
        authorize_and_sign(
            &key,
            &commitment,
            &mut session,
            &authorizer,
            &consumption,
            NOW,
            || observation(input, NOW + 2, start + Duration::from_secs(2), true),
        )
        .err(),
        Some(EvmSigningError::Approval(
            ApprovalCommitmentError::AlreadyConsumed
        ))
    );
    assert_eq!(authorizer.calls.get(), 2);
    Ok(())
}

#[test]
fn denial_cancellation_lockout_and_unavailable_authorization_fail_without_signature()
-> Result<(), Box<dyn Error>> {
    for (source, expected) in [
        (
            VaultKeyStoreError::AuthorizationDenied,
            EvmSigningError::AuthorizationDenied,
        ),
        (
            VaultKeyStoreError::AuthorizationCancelled,
            EvmSigningError::AuthorizationCancelled,
        ),
        (
            VaultKeyStoreError::AuthorizationUnavailable,
            EvmSigningError::AuthorizationUnavailable,
        ),
        (
            VaultKeyStoreError::SigningConfigurationUnavailable,
            EvmSigningError::AuthorizationUnavailable,
        ),
        (
            VaultKeyStoreError::PlatformFailure,
            EvmSigningError::AuthorizationUnavailable,
        ),
    ] {
        let start = Instant::now();
        let key = signing_key("vault-a")?;
        let mut session = unlocked_session("vault-a", start)?;
        let input = binding();
        let commitment = ApprovalCommitment::issue(input, NOW)?;
        let authorizer = FakeAuthorizer::failing(source);
        assert_eq!(
            authorize_and_sign(
                &key,
                &commitment,
                &mut session,
                &authorizer,
                &FakeConsumptionStore::default(),
                NOW,
                || { observation(input, NOW + 1, start, true) }
            )
            .err(),
            Some(expected)
        );
    }
    Ok(())
}

#[test]
fn cancelled_authorization_terminally_invalidates_the_approval() -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let key = signing_key("vault-a")?;
    let mut session = unlocked_session("vault-a", start)?;
    let input = binding();
    let commitment = ApprovalCommitment::issue(input, NOW)?;
    let consumption = FakeConsumptionStore::default();

    assert_eq!(
        authorize_and_sign(
            &key,
            &commitment,
            &mut session,
            &FakeAuthorizer::failing(VaultKeyStoreError::AuthorizationCancelled),
            &consumption,
            NOW,
            || observation(input, NOW + 1, start, true),
        )
        .err(),
        Some(EvmSigningError::AuthorizationCancelled)
    );
    assert_eq!(
        commitment.consume(input, NOW + 1, &consumption).err(),
        Some(ApprovalCommitmentError::Invalidated)
    );
    Ok(())
}

#[test]
fn post_authorization_expiry_mutation_and_backgrounding_fail_closed() -> Result<(), Box<dyn Error>>
{
    let cases = [
        (
            binding(),
            NOW + 120,
            true,
            EvmSigningError::Approval(ApprovalCommitmentError::Expired),
        ),
        (
            {
                let mut changed = binding();
                changed.signing_digest[0] ^= 1;
                changed
            },
            NOW + 1,
            true,
            EvmSigningError::Approval(ApprovalCommitmentError::SigningDigestMismatch),
        ),
        (
            binding(),
            NOW + 1,
            false,
            EvmSigningError::ApplicationBackgrounded,
        ),
    ];

    for (observed, now_unix, active, expected) in cases {
        let start = Instant::now();
        let key = signing_key("vault-a")?;
        let mut session = unlocked_session("vault-a", start)?;
        let commitment = ApprovalCommitment::issue(binding(), NOW)?;
        assert_eq!(
            authorize_and_sign(
                &key,
                &commitment,
                &mut session,
                &FakeAuthorizer::success(),
                &FakeConsumptionStore::default(),
                NOW,
                || observation(observed, now_unix, start, active),
            )
            .err(),
            Some(expected)
        );
    }
    Ok(())
}

#[test]
fn wrong_vault_account_and_locked_session_never_sign() -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let input = binding();
    let commitment = ApprovalCommitment::issue(input, NOW)?;

    let key = signing_key("vault-b")?;
    let mut session = unlocked_session("vault-a", start)?;
    assert_eq!(
        authorize_and_sign(
            &key,
            &commitment,
            &mut session,
            &FakeAuthorizer::success(),
            &FakeConsumptionStore::default(),
            NOW,
            || observation(input, NOW + 1, start, true),
        )
        .err(),
        Some(EvmSigningError::AuthorizationForDifferentVault)
    );

    let mut wrong_account = binding();
    wrong_account.account = "0xde709f2102306220921060314715629080e2fb77";
    let wrong_commitment = ApprovalCommitment::issue(wrong_account, NOW)?;
    let key = signing_key("vault-a")?;
    assert_eq!(
        authorize_and_sign(
            &key,
            &wrong_commitment,
            &mut session,
            &FakeAuthorizer::success(),
            &FakeConsumptionStore::default(),
            NOW,
            || observation(wrong_account, NOW + 1, start, true),
        )
        .err(),
        Some(EvmSigningError::ApprovalForDifferentAccount)
    );

    let mut locked = VaultSession::new(
        VaultKeyId::new("vault-a")?,
        VaultInactivityTimeout::new(Duration::from_secs(30))?,
    );
    assert_eq!(
        authorize_and_sign(
            &key,
            &commitment,
            &mut locked,
            &FakeAuthorizer::success(),
            &FakeConsumptionStore::default(),
            NOW,
            || observation(input, NOW + 1, start, true),
        )
        .err(),
        Some(EvmSigningError::VaultSession(VaultSessionError::Locked))
    );
    Ok(())
}

fn signing_key(vault_id: &str) -> Result<EvmSigningKey, Box<dyn Error>> {
    let preview =
        EvmPrivateKeyImportPreview::validate(EvmPrivateKeyInput::new(PRIVATE_KEY_ONE.to_owned())?)?;
    let acknowledgement = preview.acknowledge_recovery_warning(true)?;
    let private_key = preview.confirm(&acknowledgement)?.into_private_key();
    Ok(EvmSigningKey::from_imported(
        VaultKeyId::new(vault_id)?,
        private_key,
    )?)
}

fn unlocked_session(vault_id: &str, now: Instant) -> Result<VaultSession, Box<dyn Error>> {
    let key_id = VaultKeyId::new(vault_id)?;
    let mut session = VaultSession::new(
        key_id.clone(),
        VaultInactivityTimeout::new(Duration::from_secs(30))?,
    );
    session.unlock(
        VaultUnlockAuthorization::new(key_id, VaultEncryptionKey::generate()?),
        now,
    )?;
    Ok(session)
}

fn binding() -> ApprovalBindingInput<'static> {
    ApprovalBindingInput {
        signing_digest: [0x11; 32],
        chain_id: 8453,
        account: ACCOUNT_ONE,
        session_id: "session-01",
        expires_at_unix: NOW + 120,
        transaction_type: ApprovalTransactionType::Eip1559 {
            transaction_type: 2,
        },
        decoder_digest: [0x22; 32],
        simulation_digest: [0x33; 32],
    }
}

fn observation(
    binding: ApprovalBindingInput<'static>,
    now_unix: u64,
    now_monotonic: Instant,
    application_is_active: bool,
) -> EvmSigningObservation<'static> {
    EvmSigningObservation::new(binding, now_unix, now_monotonic, application_is_active)
}
