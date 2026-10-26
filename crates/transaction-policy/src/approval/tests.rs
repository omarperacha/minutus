use super::*;
use std::sync::{Arc, Barrier, Mutex};

const NOW: u64 = 1_800_000_000;
const ACCOUNT: &str = "0x9858effd232b4033e47d90003d41ec34ecaeda94";

#[derive(Clone, Eq, PartialEq)]
struct StoredFinalization {
    approval_id: String,
    commitment_digest: String,
    finalization: ApprovalFinalization,
}

#[derive(Default)]
struct MemoryStore {
    finalization: Mutex<Option<StoredFinalization>>,
}

impl ApprovalConsumptionStore for MemoryStore {
    fn finalize(
        &self,
        record: &ApprovalConsumptionRecord<'_>,
        finalization: ApprovalFinalization,
    ) -> ApprovalConsumptionOutcome {
        let Ok(mut stored) = self.finalization.lock() else {
            return ApprovalConsumptionOutcome::Unavailable;
        };
        let requested = StoredFinalization {
            approval_id: record.approval_id_hex(),
            commitment_digest: record.commitment_digest_hex(),
            finalization,
        };
        let Some(existing) = stored.as_ref() else {
            *stored = Some(requested);
            return ApprovalConsumptionOutcome::Finalized;
        };
        if existing.approval_id != requested.approval_id
            || existing.commitment_digest != requested.commitment_digest
        {
            return ApprovalConsumptionOutcome::IdentifierCollision;
        }
        match existing.finalization {
            ApprovalFinalization::Consumed => ApprovalConsumptionOutcome::AlreadyConsumed,
            ApprovalFinalization::Invalidated(_) => ApprovalConsumptionOutcome::Invalidated,
        }
    }
}

fn input() -> ApprovalBindingInput<'static> {
    ApprovalBindingInput {
        signing_digest: [0x11; 32],
        chain_id: 8453,
        account: ACCOUNT,
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
fn exact_live_binding_verifies_without_exposing_serializable_authority()
-> Result<(), ApprovalCommitmentError> {
    let commitment = ApprovalCommitment::issue(input(), NOW)?;
    let _verified = commitment.verify(input(), NOW + 1)?;
    assert!(!commitment.is_expired(NOW + 119));
    assert!(commitment.is_expired(NOW + 120));
    Ok(())
}

#[test]
fn one_bit_payload_mutation_invalidates_approval() -> Result<(), ApprovalCommitmentError> {
    let commitment = ApprovalCommitment::issue(input(), NOW)?;
    let mut changed = input();
    changed.signing_digest[31] ^= 1;
    assert_eq!(
        commitment.verify(changed, NOW + 1).err(),
        Some(ApprovalCommitmentError::SigningDigestMismatch)
    );
    Ok(())
}

#[test]
fn chain_account_session_expiry_and_type_substitution_fail_closed()
-> Result<(), ApprovalCommitmentError> {
    let commitment = ApprovalCommitment::issue(input(), NOW)?;

    let mut chain = input();
    chain.chain_id = 1;
    assert_eq!(
        commitment.verify(chain, NOW + 1).err(),
        Some(ApprovalCommitmentError::ChainMismatch)
    );

    let mut account = input();
    account.account = "0xde709f2102306220921060314715629080e2fb77";
    assert_eq!(
        commitment.verify(account, NOW + 1).err(),
        Some(ApprovalCommitmentError::AccountMismatch)
    );

    let mut session = input();
    session.session_id = "session-02";
    assert_eq!(
        commitment.verify(session, NOW + 1).err(),
        Some(ApprovalCommitmentError::SessionMismatch)
    );

    let mut expiry = input();
    expiry.expires_at_unix += 1;
    assert_eq!(
        commitment.verify(expiry, NOW + 1).err(),
        Some(ApprovalCommitmentError::ExpiryMismatch)
    );

    let mut transaction_type = input();
    transaction_type.transaction_type = ApprovalTransactionType::Eip712;
    assert_eq!(
        commitment.verify(transaction_type, NOW + 1).err(),
        Some(ApprovalCommitmentError::TransactionTypeMismatch)
    );
    Ok(())
}

#[test]
fn decoder_and_simulation_evidence_cannot_be_reattached() -> Result<(), ApprovalCommitmentError> {
    let commitment = ApprovalCommitment::issue(input(), NOW)?;
    let mut decoder = input();
    decoder.decoder_digest[0] ^= 1;
    assert_eq!(
        commitment.verify(decoder, NOW + 1).err(),
        Some(ApprovalCommitmentError::DecoderMismatch)
    );
    let mut simulation = input();
    simulation.simulation_digest[0] ^= 1;
    assert_eq!(
        commitment.verify(simulation, NOW + 1).err(),
        Some(ApprovalCommitmentError::SimulationMismatch)
    );
    Ok(())
}

#[test]
fn stale_policy_and_corrupt_commitment_digest_fail_closed() -> Result<(), ApprovalCommitmentError> {
    let mut stale_policy = ApprovalCommitment::issue(input(), NOW)?;
    stale_policy.binding.policy_revision += 1;
    assert_eq!(
        stale_policy.verify(input(), NOW + 1).err(),
        Some(ApprovalCommitmentError::PolicyMismatch)
    );

    let mut corrupt_digest = ApprovalCommitment::issue(input(), NOW)?;
    corrupt_digest.commitment_digest[0] ^= 1;
    assert_eq!(
        corrupt_digest.verify(input(), NOW + 1).err(),
        Some(ApprovalCommitmentError::CommitmentMismatch)
    );
    Ok(())
}

#[test]
fn expired_and_invalid_commitments_never_verify() -> Result<(), ApprovalCommitmentError> {
    let commitment = ApprovalCommitment::issue(input(), NOW)?;
    assert_eq!(
        commitment.verify(input(), NOW + 120).err(),
        Some(ApprovalCommitmentError::Expired)
    );

    let mut excessive = input();
    excessive.expires_at_unix = NOW + MAX_APPROVAL_LIFETIME_SECONDS + 1;
    assert_eq!(
        ApprovalCommitment::issue(excessive, NOW).err(),
        Some(ApprovalCommitmentError::InvalidExpiry)
    );
    let mut unsupported = input();
    unsupported.transaction_type = ApprovalTransactionType::Eip1559 {
        transaction_type: 1,
    };
    assert_eq!(
        ApprovalCommitment::issue(unsupported, NOW).err(),
        Some(ApprovalCommitmentError::UnsupportedTransactionType)
    );
    Ok(())
}

#[test]
fn commitment_digest_changes_for_every_bound_field() -> Result<(), ApprovalCommitmentError> {
    let original = ApprovalBinding::validated([0xaa; 32], input(), NOW)?;
    let original_digest = commitment_digest(&original);
    let variants = [
        ApprovalBinding {
            approval_id: [0xbb; 32],
            ..original.clone()
        },
        ApprovalBinding {
            signing_digest: [0x44; 32],
            ..original.clone()
        },
        ApprovalBinding {
            chain_id: 1,
            ..original.clone()
        },
        ApprovalBinding {
            account: "0xde709f2102306220921060314715629080e2fb77".to_owned(),
            ..original.clone()
        },
        ApprovalBinding {
            session_id: "another-session".to_owned(),
            ..original.clone()
        },
        ApprovalBinding {
            expires_at_unix: original.expires_at_unix + 1,
            ..original.clone()
        },
        ApprovalBinding {
            transaction_type: ApprovalTransactionType::Eip712,
            ..original.clone()
        },
        ApprovalBinding {
            decoder_digest: [0x55; 32],
            ..original.clone()
        },
        ApprovalBinding {
            simulation_digest: [0x66; 32],
            ..original.clone()
        },
        ApprovalBinding {
            policy_revision: original.policy_revision + 1,
            ..original.clone()
        },
    ];
    assert!(
        variants
            .iter()
            .all(|variant| commitment_digest(variant) != original_digest)
    );
    Ok(())
}

#[test]
fn canonical_framing_prevents_variable_length_field_ambiguity()
-> Result<(), ApprovalCommitmentError> {
    let first = ApprovalBinding::validated([0xaa; 32], input(), NOW)?;
    let mut second = first.clone();
    second.account = format!("{}a", &first.account[..41]);
    second.session_id = format!("a{}", first.session_id);
    assert_ne!(commitment_digest(&first), commitment_digest(&second));
    Ok(())
}

#[test]
fn approval_can_be_consumed_exactly_once() -> Result<(), ApprovalCommitmentError> {
    let commitment = ApprovalCommitment::issue(input(), NOW)?;
    let store = MemoryStore::default();
    let _consumed = commitment.consume(input(), NOW + 1, &store)?;
    assert_eq!(
        commitment.consume(input(), NOW + 2, &store).err(),
        Some(ApprovalCommitmentError::AlreadyConsumed)
    );
    Ok(())
}

#[test]
fn concurrent_consumers_produce_at_most_one_capability() -> Result<(), Box<dyn std::error::Error>> {
    let commitment = Arc::new(ApprovalCommitment::issue(input(), NOW)?);
    let store = Arc::new(MemoryStore::default());
    let barrier = Arc::new(Barrier::new(8));
    let handles = (0..8)
        .map(|_| {
            let commitment = Arc::clone(&commitment);
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                commitment.consume(input(), NOW + 1, store.as_ref()).is_ok()
            })
        })
        .collect::<Vec<_>>();
    let successes = handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .filter(|success| *success)
        .count();
    assert_eq!(successes, 1);
    Ok(())
}

#[test]
fn invalidation_is_terminal_and_cannot_become_consumption() -> Result<(), ApprovalCommitmentError> {
    let commitment = ApprovalCommitment::issue(input(), NOW)?;
    let store = MemoryStore::default();
    commitment.invalidate(ApprovalInvalidationReason::Superseded, NOW + 1, &store)?;
    assert_eq!(
        commitment.consume(input(), NOW + 2, &store).err(),
        Some(ApprovalCommitmentError::Invalidated)
    );
    commitment.invalidate(ApprovalInvalidationReason::Cancelled, NOW + 3, &store)?;
    Ok(())
}

#[test]
fn mutation_and_expiry_fail_before_durable_consumption() -> Result<(), ApprovalCommitmentError> {
    let commitment = ApprovalCommitment::issue(input(), NOW)?;
    let store = MemoryStore::default();
    let mut changed = input();
    changed.chain_id = 1;
    assert_eq!(
        commitment.consume(changed, NOW + 1, &store).err(),
        Some(ApprovalCommitmentError::ChainMismatch)
    );
    assert_eq!(
        commitment.consume(input(), NOW + 120, &store).err(),
        Some(ApprovalCommitmentError::Expired)
    );
    assert!(store.finalization.lock().is_ok_and(|value| value.is_none()));
    Ok(())
}
