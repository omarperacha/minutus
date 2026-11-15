//! Genuine synthetic signed bytes and both MPT proofs; no RPC or wallet.
use super::*;
use alloy_consensus::{Eip658Value, Receipt, SignableTransaction, Signed, TxEip1559};
use alloy_primitives::{Address, B256, Signature, TxKind, U256, keccak256};
use alloy_trie::{Nibbles, nodes::LeafNode};

type Fixture = (Header, Vec<u8>, String, String, DeploymentProof);
fn trie(bytes: &[u8]) -> (B256, Vec<String>) {
    let node = alloy_rlp::encode(LeafNode::new(
        Nibbles::unpack(alloy_rlp::encode(0_u64)),
        bytes.to_vec(),
    ));
    (
        keccak256(&node),
        vec![format!("0x{}", alloy_primitives::hex::encode(node))],
    )
}
fn fixture() -> Result<Fixture, GenericError> {
    let transaction = TxEip1559 {
        chain_id: 1,
        nonce: 3,
        gas_limit: 100_000,
        max_fee_per_gas: 10,
        max_priority_fee_per_gas: 1,
        to: TxKind::Call(Address::repeat_byte(2)),
        ..Default::default()
    };
    let payload = transaction.encoded_for_signing();
    let signature = Signature::from_scalars_and_parity(
        "0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
            .parse()
            .map_err(|_| GenericError::InvalidAbi)?,
        B256::from(U256::from(1).to_be_bytes::<32>()),
        false,
    );
    let signed = Signed::new_unhashed(transaction, signature);
    let account = format!(
        "{:#x}",
        signed
            .recover_signer()
            .map_err(|_| GenericError::InvalidAbi)?
    );
    let mut transaction = Vec::new();
    signed.eip2718_encode(&mut transaction);
    let hash = format!("{:#x}", keccak256(&transaction));
    let mut receipt = vec![2];
    receipt.extend(alloy_rlp::encode(
        Receipt {
            status: Eip658Value::Eip658(true),
            cumulative_gas_used: 50_000,
            logs: Vec::<alloy_primitives::Log>::new(),
        }
        .with_bloom(),
    ));
    let (transactions_root, transaction_proof) = trie(&transaction);
    let (receipts_root, receipt_proof) = trie(&receipt);
    let header = Header {
        transactions_root,
        receipts_root,
        number: 100,
        gas_used: 50_000,
        gas_limit: 30_000_000,
        ..Default::default()
    };
    let proof = DeploymentProof {
        block_hash: format!("{:#x}", header.hash_slow()),
        transaction_index: 0,
        transaction: format!("0x{}", alloy_primitives::hex::encode(transaction)),
        transaction_proof,
        receipt: format!("0x{}", alloy_primitives::hex::encode(receipt)),
        receipt_proof,
    };
    Ok((header, payload, account, hash, proof))
}

#[test]
fn bad_signed_body_signer_or_proof_never_becomes_a_completion_stage_failure()
-> Result<(), GenericError> {
    for mutation in 0..5 {
        let (header, mut payload, mut account, hash, mut proof) = fixture()?;
        let mut height = header.number;
        match mutation {
            0 => payload.push(0),
            1 => account = format!("{:#x}", Address::ZERO),
            2 => proof.transaction_proof.clear(),
            3 => proof.receipt_proof.clear(),
            _ => height += 1,
        }
        let checked = verify(
            EvmNetwork::EthereumMainnet,
            &header,
            &payload,
            &account,
            &hash,
            &proof,
            height,
        )
        .map_err(GenericCompletionFailure::Inclusion);
        let result: Result<(), _> =
            checked.and_then(|seal| complete(seal, |_| Err(GenericError::SourceMismatch)));
        assert!(matches!(
            result,
            Err(GenericCompletionFailure::Inclusion(
                GenericError::SnapshotMismatch
            ))
        ));
    }
    Ok(())
}

#[test]
fn valid_exact_inclusion_survives_later_source_condition_or_anchor_failure()
-> Result<(), GenericError> {
    for later_error in [
        GenericError::SourceMismatch,
        GenericError::ConditionUnsatisfied,
        GenericError::SnapshotMismatch,
        GenericError::Expired,
    ] {
        let (header, payload, account, hash, proof) = fixture()?;
        let seal = verify(
            EvmNetwork::EthereumMainnet,
            &header,
            &payload,
            &account,
            &hash,
            &proof,
            100,
        )?;
        let Err(failure) = complete::<()>(seal, |_| Err(later_error)) else {
            return Err(GenericError::ExecutionRejected);
        };
        let GenericCompletionFailure::Completion { inclusion, error } = failure else {
            return Err(GenericError::ExecutionRejected);
        };
        assert_eq!(error, later_error);
        assert_eq!(inclusion.transaction_hash(), hash);
        assert_eq!(inclusion.receipt_number, 100);
        assert_eq!(inclusion.receipt_hash, proof.block_hash);
        assert!(!inclusion.inclusion_commitment.is_empty());
        assert_eq!(
            GenericCompletionFailure::Completion { inclusion, error }.into_error(),
            later_error
        );
    }
    Ok(())
}

#[test]
fn closed_failure_metadata_preserves_the_original_legacy_error() -> Result<(), GenericError> {
    use crate::contract::generic::completion::failure::{Phase, Recorder};
    use std::sync::atomic::AtomicBool;
    for error in [
        GenericError::SnapshotMismatch,
        GenericError::ConditionUnsatisfied,
    ] {
        let (header, payload, account, hash, proof) = fixture()?;
        let seal = verify(
            EvmNetwork::EthereumMainnet,
            &header,
            &payload,
            &account,
            &hash,
            &proof,
            100,
        )?;
        let cancelled = AtomicBool::new(true);
        let control = evm_simulation::SimulationControl::start(&cancelled);
        let Err(failure) = complete_diagnosed::<()>(seal, |_, diagnostic| {
            let mut recorder = Recorder::new(&control, diagnostic);
            recorder.generic(Phase::FinalCurrentAnchor, || Err(error))
        }) else {
            return Err(GenericError::ExecutionRejected);
        };
        let diagnostic = failure
            .diagnostic()
            .ok_or(GenericError::ExecutionRejected)?;
        let text = diagnostic.to_string();
        assert!(text.contains("phase=FinalCurrentAnchor"));
        assert!(text.contains("originalControl=cancelled"));
        assert!(text.len() < 384);
        assert_eq!(failure.into_error(), error);
    }
    Ok(())
}
