use super::*;
use crate::{
    StoredDeclaredWorkflowStep, StoredEvmTransaction, StoredEvmTransactionState,
    StoredGenericWorkflowEvidence, StoredWorkflowContract, StoredWorkflowEvidenceKind,
};
use ipc_types::MessageId;

fn setup() -> Result<(ApplicationStore, StoredContractReconciliation), Box<dyn std::error::Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    store.connection.execute(
        "INSERT INTO projects(id,name,created_at,updated_at) VALUES ('p','P','now','now')",
        [],
    )?;
    store.connection.execute("INSERT INTO conversations(id,title,created_at,updated_at,project_id) VALUES ('c','C','now','now','p'),('other','Other','now','now','p')",[])?;
    store.connection.execute("INSERT INTO messages(id,conversation_id,position,role,parts_json,created_at) VALUES ('m','c',0,'user','[]','now')",[])?;
    let declaration = StoredWorkflowDeclaration {
        version: 2,
        network: "eip155:11155111".into(),
        account: format!("0x{}", "11".repeat(20)),
        contracts: vec![StoredWorkflowContract {
            id: "app".into(),
            address: format!("0x{}", "22".repeat(20)),
        }],
        steps: (0..2)
            .map(|index| StoredDeclaredWorkflowStep {
                id: format!("step{index}"),
                contract: "app".into(),
                function: "store(uint256)".into(),
                arguments: vec![serde_json::json!("42")],
                value_wei: "0".into(),
                depends_on: vec![],
                prerequisites: vec![],
                completion: vec![],
            })
            .collect(),
        goals: vec![],
    };
    let conversation = ConversationId("c".into());
    let id = store
        .create_declared_contract_workflow(&conversation, &MessageId("m".into()), &declaration)?
        .ok_or("workflow")?;
    let claim = store.claim_contract_step(&id)?.ok_or("claim")?;
    let preparation = StoredGenericWorkflowPreparation {
        scope_commitment: declaration.commitment()?,
        intent_commitment: claim.intent_commitment().into(),
        candidate_digest: format!("0x{}", "a".repeat(64)),
        signing_digest: format!("0x{}", "b".repeat(64)),
        source_commitment: "c".repeat(64),
        source_identities: vec!["d".repeat(64)],
        effects_commitment: "e".repeat(64),
        financial_commitment: "f".repeat(64),
        nonce: "0".into(),
        block_number: "1".into(),
        block_hash: format!("0x{}", "1".repeat(64)),
    };
    assert!(store.bind_generic_contract_preparation(&claim, &preparation)?);
    let nonce = store
        .claim_evm_nonce(
            &declaration.network,
            &declaration.account,
            "0",
            &preparation.candidate_digest,
            "test-owner",
        )?
        .ok_or("nonce")?;
    let hash = format!("0x{}", "2".repeat(64));
    store.reserve_evm_transaction(
        &StoredEvmTransaction {
            transaction_hash: hash.clone(),
            network: declaration.network,
            sender: declaration.account,
            nonce: "0".into(),
            candidate_digest: preparation.candidate_digest.clone(),
            state: StoredEvmTransactionState::Pending,
            block_number: None,
            block_hash: None,
            submitted_at: "now".into(),
            updated_at: "now".into(),
        },
        &nonce,
    )?;
    let evidence = StoredGenericWorkflowEvidence {
        kind: StoredWorkflowEvidenceKind::Prerequisites,
        scope_commitment: preparation.scope_commitment.clone(),
        preparation_commitment: Some(preparation.commitment()?),
        receipt: None,
        state_block_number: "1".into(),
        state_block_hash: preparation.block_hash.clone(),
        proof_commitment: "3".repeat(64),
        results: vec![],
    };
    assert!(store.mark_generic_contract_dispatched(
        &claim,
        &preparation.commitment()?,
        &hash,
        &nonce,
        &evidence
    )?);
    assert!(store.record_contract_step(
        &claim,
        ContractStepState::Dispatching,
        ContractStepState::Submitted,
        Some(&hash)
    )?);
    let snapshot = store
        .contract_reconciliation(&conversation, &hash)?
        .ok_or("snapshot")?;
    Ok((store, snapshot))
}
#[test]
fn reconciliation_never_creates_a_claim_or_releases_possible_send()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut store, snapshot) = setup()?;
    let hash = snapshot.hash.clone();
    assert!(
        store
            .contract_reconciliation(&ConversationId("other".into()), &hash)?
            .is_none()
    );
    assert!(
        store
            .contract_reconciliation(
                &ConversationId("c".into()),
                &format!("0x{}", "9".repeat(64))
            )?
            .is_none()
    );
    assert_eq!(
        store.record_contract_reconciliation(
            &snapshot,
            true,
            "2",
            &format!("0x{}", "4".repeat(64)),
            "later"
        )?,
        ContractStepState::ConfirmedUnverified
    );
    let steps = store.contract_workflow_steps(&snapshot.id)?;
    assert_eq!(steps[1].state, ContractStepState::Interrupted);
    assert!(store.claim_contract_step(&snapshot.id)?.is_none());
    assert!(
        store
            .claim_evm_nonce(
                "eip155:11155111",
                &snapshot.declaration.account,
                "0",
                &snapshot.preparation.candidate_digest,
                "another-owner"
            )?
            .is_none()
    );
    assert_eq!(store.evm_transactions()?[0].transaction_hash, hash);
    assert_eq!(
        store.evm_transactions()?[0].state,
        StoredEvmTransactionState::Confirmed
    );
    Ok(())
}
#[test]
fn restart_and_reverted_receipt_preserve_hash_and_attempt_fences()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut store, snapshot) = setup()?;
    store.interrupt_contract_workflows_after_restart()?;
    let fresh = store
        .contract_reconciliation(&ConversationId("c".into()), &snapshot.hash)?
        .ok_or("reloaded")?;
    assert_eq!(
        store.contract_workflow_steps(&fresh.id)?[0].state,
        ContractStepState::Uncertain
    );
    assert_eq!(
        store.record_contract_reconciliation(
            &fresh,
            false,
            "2",
            &format!("0x{}", "4".repeat(64)),
            "later"
        )?,
        ContractStepState::Failed
    );
    assert_eq!(
        store.contract_workflow_steps(&fresh.id)?[1].state,
        ContractStepState::Interrupted
    );
    assert!(store.claim_contract_step(&fresh.id)?.is_none());
    assert!(
        store
            .contract_workflow_for_turn(&ConversationId("c".into()), &MessageId("m".into()))?
            .is_some()
    );
    Ok(())
}
#[test]
fn changed_journal_or_non_dispatched_nonce_rejects_before_observation()
-> Result<(), Box<dyn std::error::Error>> {
    let (store, snapshot) = setup()?;
    assert!(
        store
            .connection
            .execute(
                "UPDATE evm_transactions SET candidate_digest=?1 WHERE transaction_hash=?2",
                params![format!("0x{}", "8".repeat(64)), snapshot.hash]
            )
            .is_err()
    );
    // Corruption recovery must still reject a context mismatch even if a
    // damaged database has lost its independent immutable-identity trigger.
    store
        .connection
        .execute("DROP TRIGGER evm_transaction_identity_immutable", [])?;
    store.connection.execute(
        "UPDATE evm_transactions SET candidate_digest=?1 WHERE transaction_hash=?2",
        params![format!("0x{}", "8".repeat(64)), snapshot.hash],
    )?;
    assert!(
        store
            .contract_reconciliation(&ConversationId("c".into()), &snapshot.hash)
            .is_err()
    );
    let (store, snapshot) = setup()?;
    store
        .connection
        .execute("UPDATE evm_nonce_claims SET phase='retired'", [])?;
    assert!(
        store
            .contract_reconciliation(&ConversationId("c".into()), &snapshot.hash)
            .is_err()
    );
    Ok(())
}

#[test]
fn receipt_only_reconciliation_cannot_reselect_any_historical_completion()
-> Result<(), Box<dyn std::error::Error>> {
    for receipt in [2, 3] {
        let (mut store, snapshot) = setup()?;
        let claim = super::super::ContractStepClaim {
            workflow: snapshot.id.clone(),
            index: snapshot.index,
            commitment: snapshot.preparation.intent_commitment.clone(),
        };
        for block in [2, 3] {
            let before = if block == 2 {
                ContractStepState::Submitted
            } else {
                ContractStepState::Uncertain
            };
            assert!(store.record_contract_step(
                &claim,
                before,
                ContractStepState::ConfirmedUnverified,
                Some(&snapshot.hash)
            )?);
            let evidence = StoredGenericWorkflowEvidence {
                kind: StoredWorkflowEvidenceKind::Completion,
                scope_commitment: snapshot.preparation.scope_commitment.clone(),
                preparation_commitment: Some(snapshot.preparation.commitment()?),
                receipt: Some(super::super::StoredWorkflowReceiptAnchor {
                    transaction_hash: snapshot.hash.clone(),
                    block_number: block.to_string(),
                    block_hash: format!("0x{}", block.to_string().repeat(64)),
                    inclusion_commitment: "5".repeat(64),
                }),
                state_block_number: block.to_string(),
                state_block_hash: format!("0x{}", block.to_string().repeat(64)),
                proof_commitment: block.to_string().repeat(64),
                results: vec![],
            };
            assert!(store.apply_generic_contract_completion(&claim, &evidence)?);
            if block == 2 {
                assert!(store.record_contract_step(
                    &claim,
                    ContractStepState::Verified,
                    ContractStepState::Uncertain,
                    None
                )?);
            }
        }
        assert_eq!(
            store.contract_workflow_steps(&snapshot.id)?[0].state,
            ContractStepState::Verified
        );
        assert_eq!(
            store.record_contract_reconciliation(
                &snapshot,
                true,
                &receipt.to_string(),
                &format!("0x{}", receipt.to_string().repeat(64)),
                "later"
            )?,
            ContractStepState::ConfirmedUnverified
        );
        let steps = store.contract_workflow_steps(&snapshot.id)?;
        assert!(steps[0].evidence_commitment.is_none());
        assert_eq!(steps[1].state, ContractStepState::Interrupted);
        assert_eq!(
            store
                .generic_contract_workflow_evidence(&snapshot.id, 0)?
                .iter()
                .filter(|item| item.kind == StoredWorkflowEvidenceKind::Completion)
                .count(),
            2
        );
        assert!(store.claim_contract_step(&snapshot.id)?.is_none());
    }
    Ok(())
}
