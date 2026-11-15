use super::*;
use crate::{ApplicationStore, StorageError, StoredEvmTransaction, StoredEvmTransactionState};
use ipc_types::{ConversationId, MessageId};
use serde_json::json;

const ACCOUNT: &str = "0x1111111111111111111111111111111111111111";
const TARGET: &str = "0x2222222222222222222222222222222222222222";
fn setup() -> Result<ApplicationStore, StorageError> {
    let store = ApplicationStore::open_in_memory()?;
    store.connection.execute(
        "INSERT INTO projects(id,name,created_at,updated_at) VALUES ('p','P','now','now')",
        [],
    )?;
    store.connection.execute("INSERT INTO conversations(id,title,created_at,updated_at,project_id) VALUES ('c','C','now','now','p')",[])?;
    store.connection.execute("INSERT INTO messages(id,conversation_id,position,role,parts_json,created_at) VALUES ('m','c',0,'user','[]','now')",[])?;
    Ok(store)
}
fn condition(value: &str) -> StoredWorkflowCondition {
    StoredWorkflowCondition {
        contract: "app".into(),
        read: StoredWorkflowRead::SourceField {
            path: vec![json!("remaining")],
        },
        comparison: StoredWorkflowComparison::Equal,
        expected: json!(value),
    }
}
fn declaration() -> StoredWorkflowDeclaration {
    StoredWorkflowDeclaration {
        version: 2,
        network: "eip155:8453".into(),
        account: ACCOUNT.into(),
        contracts: vec![StoredWorkflowContract {
            id: "app".into(),
            address: TARGET.into(),
        }],
        steps: vec![
            StoredDeclaredWorkflowStep {
                id: "first".into(),
                contract: "app".into(),
                function: "configure(uint256)".into(),
                arguments: vec![json!("7")],
                value_wei: "0".into(),
                depends_on: vec![],
                prerequisites: vec![],
                completion: vec![condition("7")],
            },
            StoredDeclaredWorkflowStep {
                id: "second".into(),
                contract: "app".into(),
                function: "consume(uint256)".into(),
                arguments: vec![json!("7")],
                value_wei: "0".into(),
                depends_on: vec![0],
                prerequisites: vec![condition("7")],
                completion: vec![condition("0")],
            },
            StoredDeclaredWorkflowStep {
                id: "third".into(),
                contract: "app".into(),
                function: "finish()".into(),
                arguments: vec![],
                value_wei: "0".into(),
                depends_on: vec![1],
                prerequisites: vec![condition("0")],
                completion: vec![condition("0")],
            },
        ],
        goals: vec![condition("0")],
    }
}
fn preparation(
    scope: &StoredWorkflowDeclaration,
    claim: &ContractStepClaim,
) -> Result<StoredGenericWorkflowPreparation, StorageError> {
    Ok(StoredGenericWorkflowPreparation {
        scope_commitment: scope.commitment()?,
        intent_commitment: claim.intent_commitment().into(),
        candidate_digest: format!("0x{}", format!("{:02x}", claim.index() + 10).repeat(32)),
        signing_digest: format!("0x{}", "a".repeat(64)),
        source_commitment: "b".repeat(64),
        source_identities: vec!["b".repeat(64)],
        effects_commitment: "c".repeat(64),
        financial_commitment: "d".repeat(64),
        nonce: (3 + claim.index()).to_string(),
        block_number: "42".into(),
        block_hash: format!("0x{}", "e".repeat(64)),
    })
}
fn evidence(
    scope: &StoredWorkflowDeclaration,
    preparation: &StoredGenericWorkflowPreparation,
    index: u8,
    kind: StoredWorkflowEvidenceKind,
) -> Result<StoredGenericWorkflowEvidence, StorageError> {
    let conditions = match kind {
        StoredWorkflowEvidenceKind::Completion => &scope.steps[usize::from(index)].completion,
        StoredWorkflowEvidenceKind::Prerequisites => &scope.steps[usize::from(index)].prerequisites,
        StoredWorkflowEvidenceKind::Goals => &scope.goals,
    };
    Ok(StoredGenericWorkflowEvidence {
        kind,
        scope_commitment: scope.commitment()?,
        preparation_commitment: if kind == StoredWorkflowEvidenceKind::Goals {
            None
        } else {
            Some(preparation.commitment()?)
        },
        receipt: if kind == StoredWorkflowEvidenceKind::Completion {
            Some(StoredWorkflowReceiptAnchor {
                transaction_hash: format!("0x{}", format!("{:02x}", index + 20).repeat(32)),
                block_number: "43".into(),
                block_hash: format!("0x{}", "f".repeat(64)),
                inclusion_commitment: "1".repeat(64),
            })
        } else {
            None
        },
        state_block_number: "43".into(),
        state_block_hash: format!("0x{}", "f".repeat(64)),
        proof_commitment: "2".repeat(64),
        results: conditions
            .iter()
            .map(|condition| StoredWorkflowConditionResult {
                condition: condition.clone(),
                value: condition.expected.clone(),
            })
            .collect(),
    })
}

#[test]
fn consumed_historical_condition_does_not_block_a_later_declared_requirement()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let declaration = declaration();
    let id = store
        .create_declared_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &declaration,
        )?
        .ok_or("missing workflow")?;
    for index in 0..3 {
        let claim = store.claim_contract_step(&id)?.ok_or("missing claim")?;
        assert_eq!(claim.index(), index);
        let prepared = preparation(&declaration, &claim)?;
        assert!(store.bind_generic_contract_preparation(&claim, &prepared)?);
        assert!(!store.bind_generic_contract_preparation(&claim, &prepared)?);
        let nonce = store
            .claim_evm_nonce(
                &declaration.network,
                ACCOUNT,
                &prepared.nonce,
                &prepared.candidate_digest,
                &format!("owner-{index}"),
            )?
            .ok_or("missing nonce")?;
        let record = StoredEvmTransaction {
            transaction_hash: format!("0x{}", format!("{:02x}", index + 20).repeat(32)),
            network: declaration.network.clone(),
            sender: ACCOUNT.into(),
            nonce: prepared.nonce.clone(),
            candidate_digest: prepared.candidate_digest.clone(),
            state: StoredEvmTransactionState::Pending,
            block_number: None,
            block_hash: None,
            submitted_at: "now".into(),
            updated_at: "now".into(),
        };
        store.reserve_evm_transaction(&record, &nonce)?;
        let prerequisites = evidence(
            &declaration,
            &prepared,
            index,
            StoredWorkflowEvidenceKind::Prerequisites,
        )?;
        assert!(store.mark_generic_contract_dispatched(
            &claim,
            &prepared.commitment()?,
            &record.transaction_hash,
            &nonce,
            &prerequisites
        )?);
        assert!(store.record_contract_step(
            &claim,
            ContractStepState::Dispatching,
            ContractStepState::Submitted,
            Some(&record.transaction_hash)
        )?);
        assert!(store.record_contract_step(
            &claim,
            ContractStepState::Submitted,
            ContractStepState::ConfirmedUnverified,
            Some(&record.transaction_hash)
        )?);
        assert!(store.apply_generic_contract_completion(
            &claim,
            &evidence(
                &declaration,
                &prepared,
                index,
                StoredWorkflowEvidenceKind::Completion
            )?
        )?);
        assert!(!store.apply_generic_contract_completion(
            &claim,
            &evidence(
                &declaration,
                &prepared,
                index,
                StoredWorkflowEvidenceKind::Completion
            )?
        )?);
    }
    assert!(
        store
            .contract_workflow_steps(&id)?
            .iter()
            .all(|step| step.state == ContractStepState::Verified)
    );
    let first = store.generic_contract_workflow_evidence(&id, 0)?;
    assert_eq!(
        first.last().ok_or("missing evidence")?.results[0].value,
        json!("7")
    );
    let last = store
        .generic_contract_preparation(&id, 2)?
        .ok_or("missing preparation")?;
    assert!(store.record_generic_contract_goals(
        &id,
        &evidence(&declaration, &last, 2, StoredWorkflowEvidenceKind::Goals)?
    )?);
    Ok(())
}

#[test]
fn declarations_fence_aliases_dependencies_and_legacy_downgrade()
-> Result<(), Box<dyn std::error::Error>> {
    let mut declaration = declaration();
    declaration.steps[0].completion.clear();
    assert!(declaration.commitment().is_err());
    declaration = super::generic_tests::declaration();
    declaration.steps[1].depends_on = vec![1];
    assert!(declaration.commitment().is_err());
    declaration = super::generic_tests::declaration();
    declaration.steps[0].completion[0].contract = "foreign".into();
    assert!(declaration.commitment().is_err());
    declaration = super::generic_tests::declaration();
    let mut store = setup()?;
    let id = store
        .create_declared_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &declaration,
        )?
        .ok_or("missing workflow")?;
    assert!(
        store
            .create_declared_contract_workflow(
                &ConversationId("c".into()),
                &MessageId("m".into()),
                &declaration
            )?
            .is_none()
    );
    assert_eq!(store.contract_workflow_declaration(&id)?, Some(declaration));
    assert!(
        store
            .create_contract_workflow(
                &ConversationId("c".into()),
                &MessageId("m".into()),
                &"a".repeat(64),
                &["b".repeat(64)]
            )?
            .is_none()
    );
    assert!(!store.claim_chat_transfer(&ConversationId("c".into()), &MessageId("m".into()))?);
    assert!(
        store
            .connection
            .execute(
                "UPDATE contract_workflow_declarations SET declaration_json='{}'",
                []
            )
            .is_err()
    );
    assert!(store.contract_workflow_scope(&id).is_err());
    store
        .connection
        .execute("DELETE FROM contract_workflow_declarations", [])?;
    assert!(store.contract_workflow_steps(&id).is_err());
    Ok(())
}

#[test]
fn generic_gate_and_nonce_fence_roll_back_together_on_storage_fault()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let declaration = declaration();
    let id = store
        .create_declared_contract_workflow(
            &ConversationId("c".into()),
            &MessageId("m".into()),
            &declaration,
        )?
        .ok_or("workflow")?;
    let claim = store.claim_contract_step(&id)?.ok_or("claim")?;
    let prepared = preparation(&declaration, &claim)?;
    assert!(store.bind_generic_contract_preparation(&claim, &prepared)?);
    let nonce = store
        .claim_evm_nonce(
            &declaration.network,
            ACCOUNT,
            &prepared.nonce,
            &prepared.candidate_digest,
            "fault-owner",
        )?
        .ok_or("nonce")?;
    let record = StoredEvmTransaction {
        transaction_hash: format!("0x{}", "14".repeat(32)),
        network: declaration.network.clone(),
        sender: ACCOUNT.into(),
        nonce: prepared.nonce.clone(),
        candidate_digest: prepared.candidate_digest.clone(),
        state: StoredEvmTransactionState::Pending,
        block_number: None,
        block_hash: None,
        submitted_at: "now".into(),
        updated_at: "now".into(),
    };
    store.reserve_evm_transaction(&record, &nonce)?;
    store.connection.execute_batch("CREATE TRIGGER reject_generic_dispatch BEFORE UPDATE ON contract_workflow_steps WHEN NEW.state='dispatching' BEGIN SELECT RAISE(ABORT,'injected storage fault'); END;")?;
    assert!(
        store
            .mark_generic_contract_dispatched(
                &claim,
                &prepared.commitment()?,
                &record.transaction_hash,
                &nonce,
                &evidence(
                    &declaration,
                    &prepared,
                    0,
                    StoredWorkflowEvidenceKind::Prerequisites
                )?
            )
            .is_err()
    );
    assert_eq!(
        store.contract_workflow_steps(&id)?[0].state,
        ContractStepState::Reviewing
    );
    assert!(store.generic_contract_workflow_evidence(&id, 0)?.is_empty());
    assert_eq!(
        store.connection.query_row(
            "SELECT phase FROM evm_nonce_claims WHERE owner_token='fault-owner'",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "reserved"
    );
    Ok(())
}

#[test]
fn schema_twenty_four_upgrade_retains_original_scoped_history()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = setup()?;
    let old = StoredWorkflowScope {
        version: 1,
        network: "eip155:8453".into(),
        account: ACCOUNT.into(),
        steps: vec![StoredWorkflowIntent {
            target: TARGET.into(),
            profile_id: "historical".into(),
            profile_revision: 1,
            profile_commitment: "a".repeat(64),
            operation: "oldAction(uint256)".into(),
            arguments: vec![json!("7")],
            value_wei: "0".into(),
        }],
    };
    let id = store
        .create_scoped_contract_workflow(&ConversationId("c".into()), &MessageId("m".into()), &old)?
        .ok_or("old scope")?;
    store.connection.execute_batch("DROP TRIGGER contract_scope_excludes_generic; DROP TABLE contract_workflow_evidence; DROP TABLE contract_workflow_preparations; DROP TABLE contract_workflow_declarations; DELETE FROM schema_migrations WHERE version=25; PRAGMA user_version=24;")?;
    crate::migration::initialize(&mut store.connection)?;
    assert_eq!(store.contract_workflow_scope(&id)?, Some(old));
    assert!(store.contract_workflow_declaration(&id)?.is_none());
    assert_eq!(
        store.contract_workflow_steps(&id)?[0].state,
        ContractStepState::Planned
    );
    assert!(
        store
            .create_declared_contract_workflow(
                &ConversationId("c".into()),
                &MessageId("m".into()),
                &declaration()
            )?
            .is_none()
    );
    Ok(())
}
