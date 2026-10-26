//! Durable composition adapter for single-use transaction approvals.

use app_storage::{
    ApplicationStore, StoredApprovalFinalization, StoredApprovalFinalizationOutcome,
};
use transaction_policy::{
    ApprovalConsumptionOutcome, ApprovalConsumptionRecord, ApprovalConsumptionStore,
    ApprovalFinalization,
};

use crate::AppCore;

/// Adapts ordinary `SQLite` persistence to the transaction-policy consumption port.
///
/// The adapter stores only public binding metadata. It has no wallet, signing or
/// approval-issuance authority, and any storage failure fails signing closed.
pub struct DurableApprovalConsumption<'store> {
    store: &'store ApplicationStore,
}

impl<'store> DurableApprovalConsumption<'store> {
    /// Creates a durable finalization adapter over the application database.
    #[must_use]
    pub const fn new(store: &'store ApplicationStore) -> Self {
        Self { store }
    }
}

impl ApprovalConsumptionStore for DurableApprovalConsumption<'_> {
    fn finalize(
        &self,
        record: &ApprovalConsumptionRecord<'_>,
        finalization: ApprovalFinalization,
    ) -> ApprovalConsumptionOutcome {
        finalize_with_store(self.store, record, finalization)
    }
}

impl ApprovalConsumptionStore for AppCore {
    fn finalize(
        &self,
        record: &ApprovalConsumptionRecord<'_>,
        finalization: ApprovalFinalization,
    ) -> ApprovalConsumptionOutcome {
        self.with_storage(|store| Ok(finalize_with_store(store, record, finalization)))
            .unwrap_or(ApprovalConsumptionOutcome::Unavailable)
    }
}

fn finalize_with_store(
    store: &ApplicationStore,
    record: &ApprovalConsumptionRecord<'_>,
    finalization: ApprovalFinalization,
) -> ApprovalConsumptionOutcome {
    let stored = StoredApprovalFinalization {
        approval_id: record.approval_id_hex(),
        commitment_digest: record.commitment_digest_hex(),
        signing_digest: record.signing_digest_hex(),
        chain_id: record.chain_id().to_string(),
        account: record.account().to_owned(),
        session_id: record.session_id().to_owned(),
        expires_at_unix: record.expires_at_unix().to_string(),
        transaction_type: record.transaction_type().to_owned(),
        policy_revision: record.policy_revision().to_string(),
        disposition: finalization.disposition().to_owned(),
        invalidation_reason: finalization.reason().map(str::to_owned),
        finalized_at_unix: record.finalized_at_unix().to_string(),
    };
    match store.finalize_transaction_approval(&stored) {
        Ok(StoredApprovalFinalizationOutcome::Finalized) => ApprovalConsumptionOutcome::Finalized,
        Ok(StoredApprovalFinalizationOutcome::AlreadyConsumed) => {
            ApprovalConsumptionOutcome::AlreadyConsumed
        }
        Ok(StoredApprovalFinalizationOutcome::Invalidated) => {
            ApprovalConsumptionOutcome::Invalidated
        }
        Ok(StoredApprovalFinalizationOutcome::IdentifierCollision) => {
            ApprovalConsumptionOutcome::IdentifierCollision
        }
        Err(_) => ApprovalConsumptionOutcome::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use tempfile::NamedTempFile;
    use transaction_policy::{
        ApprovalBindingInput, ApprovalCommitment, ApprovalCommitmentError, ApprovalTransactionType,
    };

    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn binding() -> ApprovalBindingInput<'static> {
        ApprovalBindingInput {
            signing_digest: [0x11; 32],
            chain_id: 8453,
            account: "0x9858effd232b4033e47d90003d41ec34ecaeda94",
            session_id: "session-01",
            expires_at_unix: NOW + 120,
            transaction_type: ApprovalTransactionType::Eip1559 {
                transaction_type: 2,
            },
            decoder_digest: [0x22; 32],
            simulation_digest: [0x33; 32],
        }
    }

    #[test]
    fn consumed_approval_remains_consumed_after_database_reopen() -> Result<(), Box<dyn Error>> {
        let database = NamedTempFile::new()?;
        let commitment = ApprovalCommitment::issue(binding(), NOW)?;
        {
            let store = ApplicationStore::open(database.path())?;
            let durable = DurableApprovalConsumption::new(&store);
            let _consumed = commitment.consume(binding(), NOW + 1, &durable)?;
        }

        let reopened = ApplicationStore::open(database.path())?;
        let durable = DurableApprovalConsumption::new(&reopened);
        assert_eq!(
            commitment.consume(binding(), NOW + 2, &durable).err(),
            Some(ApprovalCommitmentError::AlreadyConsumed)
        );
        Ok(())
    }
}
