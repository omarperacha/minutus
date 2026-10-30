//! These tests exercise private structural fact comparison only. They never
//! manufacture a chain certificate or credit a verified workflow step.
use super::*;
use app_storage::{StoredWorkflowIntent, StoredWorkflowScope};
use serde_json::json;
const ACCOUNT: &str = "0x1111111111111111111111111111111111111111";
const TARGET: &str = "0x2222222222222222222222222222222222222222";
const SPENDER: &str = "0x3333333333333333333333333333333333333333";
fn fixture() -> Result<(ContractWorkflowScope, StoredWorkflowCandidate), StorageError> {
    let scope = ContractWorkflowScope::new(StoredWorkflowScope {
        version: 1,
        network: "eip155:11155111".into(),
        account: ACCOUNT.into(),
        steps: vec![StoredWorkflowIntent {
            target: TARGET.into(),
            profile_id: "fixture-v1".into(),
            profile_revision: 1,
            profile_commitment: "a".repeat(64),
            operation: "approve(address,uint256)".into(),
            arguments: vec![json!(SPENDER), json!("7")],
            value_wei: "0".into(),
        }],
    })?;
    let observer = StoredContractObserver {
        profile_id: "fixture-v1".into(),
        profile_revision: 1,
        profile_commitment: "a".repeat(64),
        predicate: StoredObserverPredicate::Allowance {
            token: TARGET.into(),
            owner: ACCOUNT.into(),
            spender: SPENDER.into(),
            amount: "7".into(),
        },
    };
    let binding = StoredWorkflowCandidate {
        intent_commitment: scope.intent().step_commitments()?[0].clone(),
        candidate_digest: format!("0x{}", "1".repeat(64)),
        signing_digest: format!("0x{}", "2".repeat(64)),
        source_commitment: "b".repeat(64),
        effects_commitment: "c".repeat(64),
        financial_commitment: "d".repeat(64),
        nonce: "0".into(),
        block_number: "42".into(),
        block_hash: format!("0x{}", "3".repeat(64)),
        observers: vec![observer],
    };
    Ok((scope, binding))
}
fn facts() -> ReceiptFacts {
    ReceiptFacts {
        fresh: true,
        candidate_digest: format!("0x{}", "1".repeat(64)),
        network: chain_evm::EvmNetwork::EthereumSepolia,
        account: ACCOUNT.into(),
        target: TARGET.into(),
        profile_id: "fixture-v1".into(),
        profile_commitment: "a".repeat(64),
        transaction_hash: format!("0x{}", "4".repeat(64)),
        receipt_block_number: 43,
        receipt_block_hash: format!("0x{}", "5".repeat(64)),
        state_block_number: 44,
        state_block_hash: format!("0x{}", "6".repeat(64)),
        receipt_commitment: "e".repeat(64),
        proof_commitment: "f".repeat(64),
        observations: vec![(
            ContractPredicate::Allowance {
                owner: ACCOUNT.into(),
                spender: SPENDER.into(),
                amount: "7".into(),
            },
            "7".into(),
        )],
    }
}
#[test]
fn only_exact_fresh_candidate_scope_source_and_ordered_goal_facts_match() -> Result<(), StorageError>
{
    let (scope, binding) = fixture()?;
    assert!(
        from_facts(&scope, 0, &binding, facts())?
            .commitment()
            .is_ok()
    );
    for mutation in 0..8 {
        let mut bad = facts();
        match mutation {
            0 => bad.fresh = false,
            1 => bad.candidate_digest = format!("0x{}", "9".repeat(64)),
            2 => bad.network = chain_evm::EvmNetwork::BaseSepolia,
            3 => bad.account = SPENDER.into(),
            4 => bad.target = SPENDER.into(),
            5 => bad.profile_id = "unknown-v2".into(),
            6 => bad.profile_commitment = "9".repeat(64),
            _ => {
                bad.observations[0].0 = ContractPredicate::Allowance {
                    owner: ACCOUNT.into(),
                    spender: SPENDER.into(),
                    amount: "8".into(),
                }
            }
        }
        assert!(from_facts(&scope, 0, &binding, bad).is_err());
    }
    Ok(())
}
#[test]
fn token_identity_cannot_be_erased_and_raw_application_words_never_qualify()
-> Result<(), StorageError> {
    let (scope, mut binding) = fixture()?;
    if let StoredObserverPredicate::Allowance { token, .. } = &mut binding.observers[0].predicate {
        *token = SPENDER.into();
    }
    assert!(from_facts(&scope, 0, &binding, facts()).is_err());
    binding.observers[0].predicate = StoredObserverPredicate::ApplicationWord {
        contract: TARGET.into(),
        slot: format!("0x{}", "0".repeat(64)),
        value: format!("0x{}", "1".repeat(64)),
    };
    assert!(from_facts(&scope, 0, &binding, facts()).is_err());
    Ok(())
}

#[test]
fn prerequisite_cohorts_reject_mixed_heights_hashes_and_expiry_before_persistence()
-> Result<(), StorageError> {
    validate_receipt_cohort(&[])?;
    validate_receipt_cohort(&[facts(), facts()])?;
    for mutation in 0..4 {
        let mut second = facts();
        match mutation {
            0 => second.state_block_number += 1,
            1 => second.state_block_hash = format!("0x{}", "7".repeat(64)),
            2 => second.fresh = false,
            _ => {
                second.state_block_number += 1;
                second.state_block_hash = format!("0x{}", "7".repeat(64));
                second.observations[0] = (
                    ContractPredicate::Allowance {
                        owner: ACCOUNT.into(),
                        spender: SPENDER.into(),
                        amount: "0".into(),
                    },
                    "0".into(),
                );
            }
        }
        assert!(validate_receipt_cohort(&[facts(), second]).is_err());
    }
    Ok(())
}

#[test]
fn foreign_hash_in_second_prerequisite_rejects_the_entire_read_only_preflight()
-> Result<(), StorageError> {
    let first = facts();
    let mut second = facts();
    second.transaction_hash = format!("0x{}", "8".repeat(64));
    let steps = vec![
        app_storage::ContractWorkflowStep {
            index: 0,
            commitment: "a".repeat(64),
            state: app_storage::ContractStepState::Verified,
            transaction_hash: Some(first.transaction_hash.clone()),
            evidence_commitment: Some("b".repeat(64)),
        },
        app_storage::ContractWorkflowStep {
            index: 1,
            commitment: "c".repeat(64),
            state: app_storage::ContractStepState::Verified,
            transaction_hash: Some(second.transaction_hash.clone()),
            evidence_commitment: Some("d".repeat(64)),
        },
    ];
    validate_stored_prerequisites(&steps, &[first, second])?;
    let mut foreign = facts();
    foreign.transaction_hash = format!("0x{}", "9".repeat(64));
    // This batch validator has no storage mutation port. It rejects the second
    // hash before the caller can obtain any refresh updates for the first step.
    assert!(validate_stored_prerequisites(&steps, &[facts(), foreign]).is_err());
    Ok(())
}
