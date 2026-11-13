use super::*;
use crate::{
    AccountMetadata, AppVaultEntry, PasswordVaultWork, RecoverySeed, VaultInactivityTimeout,
    derive_default_evm_account,
};
use std::{
    cell::Cell,
    time::{Duration, Instant},
};
use transaction_policy::{
    ApprovalBindingInput, ApprovalConsumptionOutcome, ApprovalConsumptionRecord,
    ApprovalFinalization, ApprovalTransactionType,
};

#[derive(Default)]
struct FinalizationStore(Cell<Option<ApprovalFinalization>>);
impl ApprovalConsumptionStore for FinalizationStore {
    fn finalize(
        &self,
        _: &ApprovalConsumptionRecord<'_>,
        value: ApprovalFinalization,
    ) -> ApprovalConsumptionOutcome {
        match self.0.get() {
            None => {
                self.0.set(Some(value));
                ApprovalConsumptionOutcome::Finalized
            }
            Some(ApprovalFinalization::Consumed) => ApprovalConsumptionOutcome::AlreadyConsumed,
            Some(ApprovalFinalization::Invalidated(_)) => ApprovalConsumptionOutcome::Invalidated,
        }
    }
}

struct Fixture {
    work: PasswordVaultWork,
    job: PasswordVaultJob,
    document: AppVaultDocument,
    password: WalletPassword,
    entry: VaultKeyId,
    account: AccountMetadata,
    session: VaultSession,
}
impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let work = PasswordVaultWork::new();
        let job = work.begin()?;
        let password = WalletPassword::new("synthetic fresh attempt password".into())?;
        let entry = VaultKeyId::new("selected")?;
        let seed = RecoverySeed::new([7; 64]);
        let account = derive_default_evm_account(&seed)?.metadata("synthetic", 8453)?;
        let document = AppVaultDocument::create(
            VaultKeyId::new("root")?,
            AppVaultEntry::recovery(entry.clone(), &seed, account.clone())?,
            &password,
            &job,
        )?;
        let mut session = VaultSession::new(
            entry.clone(),
            VaultInactivityTimeout::new(Duration::from_mins(5))?,
        );
        document.authenticate(&password, &job)?.activate(
            &entry,
            &mut session,
            Instant::now(),
            &job,
        )?;
        Ok(Self {
            work,
            job,
            document,
            password,
            entry,
            account,
            session,
        })
    }
}
fn binding(account: &str) -> ApprovalBindingInput<'_> {
    ApprovalBindingInput {
        signing_digest: [0x11; 32],
        chain_id: 8453,
        account,
        session_id: "exact-review",
        expires_at_unix: 1120,
        transaction_type: ApprovalTransactionType::Eip1559 {
            transaction_type: 2,
        },
        decoder_digest: [0x22; 32],
        simulation_digest: [0x33; 32],
    }
}

#[test]
fn current_fresh_app_attempt_signs_once_without_a_foreground_requirement()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let account = fixture.account.address().to_owned();
    let binding = binding(&account);
    let commitment = ApprovalCommitment::issue(binding, 1000)?;
    let store = FinalizationStore::default();
    let authentication =
        fixture
            .document
            .authenticate_signing(&fixture.entry, &fixture.password, &fixture.job)?;
    assert!(matches!(
        authentication.authorization_context(),
        AuthorizationContext::FreshAppAttempt
    ));
    let signature = authentication.sign(
        &fixture.document,
        &fixture.job,
        &commitment,
        &mut fixture.session,
        &store,
        1001,
        || EvmSigningObservation::new(binding, 1002, Instant::now(), true),
    )?;
    signature.verify(&binding.signing_digest, binding.account)?;
    assert_eq!(store.0.get(), Some(ApprovalFinalization::Consumed));
    let second =
        fixture
            .document
            .authenticate_signing(&fixture.entry, &fixture.password, &fixture.job)?;
    assert!(
        second
            .sign(
                &fixture.document,
                &fixture.job,
                &commitment,
                &mut fixture.session,
                &store,
                1001,
                || EvmSigningObservation::new(binding, 1002, Instant::now(), true)
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn changed_live_context_or_cancelled_job_cannot_use_fresh_authentication()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let account = fixture.account.address().to_owned();
    let binding = binding(&account);
    let commitment = ApprovalCommitment::issue(binding, 1000)?;
    let authentication =
        fixture
            .document
            .authenticate_signing(&fixture.entry, &fixture.password, &fixture.job)?;
    assert_eq!(
        authentication
            .sign(
                &fixture.document,
                &fixture.job,
                &commitment,
                &mut fixture.session,
                &FinalizationStore::default(),
                1001,
                || EvmSigningObservation::new(binding, 1002, Instant::now(), false)
            )
            .err(),
        Some(EvmSigningError::AuthorizationCancelled)
    );
    let authentication =
        fixture
            .document
            .authenticate_signing(&fixture.entry, &fixture.password, &fixture.job)?;
    fixture.work.cancel();
    let commitment = ApprovalCommitment::issue(binding, 1000)?;
    assert_eq!(
        authentication
            .sign(
                &fixture.document,
                &fixture.job,
                &commitment,
                &mut fixture.session,
                &FinalizationStore::default(),
                1001,
                || EvmSigningObservation::new(binding, 1002, Instant::now(), true)
            )
            .err(),
        Some(EvmSigningError::AuthorizationCancelled)
    );
    Ok(())
}

#[test]
fn fresh_app_policy_still_rejects_changed_bindings_and_expired_approvals()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let account = fixture.account.address().to_owned();
    let binding = binding(&account);
    for expired in [false, true] {
        let commitment = ApprovalCommitment::issue(binding, 1000)?;
        let authentication = fixture.document.authenticate_signing(
            &fixture.entry,
            &fixture.password,
            &fixture.job,
        )?;
        let mut observation = binding;
        if !expired {
            observation.signing_digest = [0x99; 32];
        }
        let result = authentication.sign(
            &fixture.document,
            &fixture.job,
            &commitment,
            &mut fixture.session,
            &FinalizationStore::default(),
            1001,
            || {
                EvmSigningObservation::new(
                    observation,
                    if expired { 1120 } else { 1002 },
                    Instant::now(),
                    true,
                )
            },
        );
        assert!(matches!(result, Err(EvmSigningError::Approval(_))));
    }
    Ok(())
}
