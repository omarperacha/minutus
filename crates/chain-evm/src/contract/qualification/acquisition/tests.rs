use super::*;
use alloy_consensus::Eip658Value;
use alloy_primitives::Log;

#[test]
fn proofs_cover_rlp_index_boundaries_and_every_neighbor() -> Result<(), QualificationError> {
    for count in [1, 2, 3, 16, 64, 129] {
        let values: Vec<_> = (0..count)
            .map(|index| vec![u8::try_from(index).unwrap_or(0); index + 128])
            .collect();
        let root = alloy_trie::root::ordered_trie_root_encoded(&values);
        for index in 0..count {
            let proof = trie_proof(
                &values,
                u64::try_from(index).map_err(|_| QualificationError::InvalidIntent)?,
                root,
            );
            assert!(proof.is_ok(), "count={count},index={index}");
            let proof = proof?;
            assert!(!proof.is_empty());
            let mut changed = values.clone();
            changed[index].push(0);
            assert!(
                trie_proof(
                    &changed,
                    u64::try_from(index).map_err(|_| QualificationError::InvalidIntent)?,
                    root
                )
                .is_err()
            );
        }
        assert!(
            trie_proof(
                &values,
                u64::try_from(count).map_err(|_| QualificationError::InvalidIntent)?,
                root
            )
            .is_err()
        );
    }
    Ok(())
}
fn receipt() -> Value {
    let receipt = Receipt {
        status: Eip658Value::Eip658(true),
        cumulative_gas_used: 21_000,
        logs: Vec::<Log>::new(),
    }
    .with_bloom();
    let mut value = serde_json::to_value(receipt).unwrap_or(Value::Null);
    value["type"] = json!("0x2");
    value
}
#[test]
fn raw_type_bloom_and_deposit_fields_are_part_of_receipt_encoding() -> Result<(), QualificationError>
{
    let value = receipt();
    let bytes = receipt_bytes(EvmNetwork::EthereumMainnet, &[2], &value)?;
    assert_eq!(bytes.first(), Some(&2));
    for bad in [0, 1, 3, 4, 0x7e, 0xff] {
        assert!(receipt_bytes(EvmNetwork::EthereumMainnet, &[bad], &value).is_err());
    }
    let mut deposit = value.clone();
    deposit["type"] = json!("0x7e");
    deposit["depositNonce"] = json!("0x0");
    deposit["depositReceiptVersion"] = json!("0x1");
    let bytes = receipt_bytes(EvmNetwork::Mainnet, &[0x7e], &deposit)?;
    let expected = base_consensus::DepositReceipt {
        inner: Receipt {
            status: Eip658Value::Eip658(true),
            cumulative_gas_used: 21_000,
            logs: vec![],
        },
        deposit_nonce: Some(0),
        deposit_receipt_version: Some(1),
    }
    .with_bloom();
    assert_eq!(bytes, [vec![0x7e], alloy_rlp::encode(expected)].concat());
    assert!(receipt_bytes(EvmNetwork::EthereumMainnet, &[0x7e], &deposit).is_err());
    deposit.as_object_mut().map(|v| v.remove("depositNonce"));
    assert!(receipt_bytes(EvmNetwork::Mainnet, &[0x7e], &deposit).is_err());
    let mut ordinary = value;
    ordinary["depositNonce"] = json!("0x1");
    assert!(receipt_bytes(EvmNetwork::Mainnet, &[2], &ordinary).is_err());
    Ok(())
}

#[test]
fn pinned_verifier_rejects_tiny_inline_root_outside_transaction_receipt_domain() {
    // Alloy0.9.9 verify_proof compares RlpNode::from_rlp(short root) to a hashed
    // root reference, so valid <32-byte roots are unsupported at this pin. Real
    // signed envelopes and bloom-bearing receipts exceed32 bytes. Keep that
    // verifier strict, and use domain-valid long values in the boundary corpus.
    let values = vec![vec![0u8]];
    let root = alloy_trie::root::ordered_trie_root_encoded(&values);
    assert!(trie_proof(&values, 0, root).is_err());
}
mod rpc_integration;

#[test]
fn receipt_metadata_quantities_are_canonical_and_bounded() {
    for value in ["0x", "0x00", "0x+1", "0xA", "0x10000000000000000"] {
        assert!(quantity(&json!({"index":value}), "index").is_err());
    }
    assert_eq!(quantity(&json!({"index":"0x0"}), "index"), Ok(0));
}

#[test]
fn native_base_aa_neighbor_receipt_uses_consensus_body_only() -> Result<(), QualificationError> {
    let mut value = receipt();
    value["type"] = json!("0x79");
    value["phaseStatuses"] = json!(["0x1", "0x0"]);
    let bytes = receipt_bytes(EvmNetwork::BaseMainnet, &[0x79], &value)?;
    let decoded: ReceiptWithBloom<Receipt> =
        serde_json::from_value(value.clone()).map_err(|_| QualificationError::SnapshotMismatch)?;
    let native = ReceiptWithBloom {
        receipt: base_consensus::Eip8130Receipt::new(decoded.receipt, vec![1, 0]),
        logs_bloom: decoded.logs_bloom,
    };
    assert_eq!(bytes, [vec![0x79], alloy_rlp::encode(native)].concat());
    assert!(receipt_bytes(EvmNetwork::EthereumMainnet, &[0x79], &value).is_err());
    value["depositNonce"] = json!("0x1");
    assert!(receipt_bytes(EvmNetwork::BaseMainnet, &[0x79], &value).is_err());
    Ok(())
}
