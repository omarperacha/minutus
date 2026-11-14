//! Captured public proof regression; no RPC, wallet, signing or fresh canonicality.
//! Only the private owner test can invoke pure verification on a supplied header.
use super::*;
use alloy_consensus::{Eip658Value, Receipt, ReceiptWithBloom, TxEip1559};
use alloy_rlp::Decodable;
use serde_json::Value;

const HASH: &str = "0x543e7af2dce6b9aea0b81a40fa8decf08ab940e52d3f1a1a194127b93aed7d80";
const ACCOUNT: &str = "0x740741239ad5684428a392fc8e7aafaf6808151c";
const DIGEST: &str = "0xd13ae6b9d4e7768828c2b37f9dc8d3deb11adb6a9ad95a3a28313f66c7e4f849";

fn fixture() -> Result<(Expected, Header, DeploymentProof), Box<dyn std::error::Error>> {
    let header: Value =
        serde_json::from_str(include_str!("fixtures/sepolia-amsterdam-header.json"))?;
    Ok((
        Expected::parse(EvmNetwork::EthereumSepolia, HASH, ACCOUNT, "1", DIGEST)?,
        serde_json::from_value(header["result"].clone())?,
        serde_json::from_str(include_str!("fixtures/sepolia-amsterdam-proof.json"))?,
    ))
}

#[test]
fn captured_amsterdam_receipt_proves_both_roots_and_exact_retained_body()
-> Result<(), Box<dyn std::error::Error>> {
    let (expected, header, proof) = fixture()?;
    assert_eq!(header.number, 12_126_246);
    assert_eq!(header.gas_used, 96_694_775);
    assert_eq!(proof.transaction_index, 105);
    let raw: Value = serde_json::from_str(include_str!("fixtures/sepolia-amsterdam-receipt.json"))?;
    let receipt: ReceiptWithBloom<Receipt> = serde_json::from_value(raw["result"].clone())?;
    assert_eq!(receipt.receipt.cumulative_gas_used, 138_041_977);
    assert!(receipt.receipt.cumulative_gas_used > header.gas_used);
    let raw: Value =
        serde_json::from_str(include_str!("fixtures/sepolia-amsterdam-transaction.json"))?;
    let transaction: TxEip1559 = serde_json::from_value(raw["result"].clone())?;
    assert_eq!(
        format!("{:#x}", keccak256(transaction.encoded_for_signing())),
        DIGEST
    );
    let verified = expected.verify(&header, &proof)?;
    assert!(verified.succeeded());
    assert_eq!(verified.transaction_hash(), HASH);
    assert_eq!(verified.account(), ACCOUNT);
    assert_eq!(verified.nonce(), "1");
    assert_eq!(verified.signing_digest(), DIGEST);
    assert_eq!(verified.block_hash(), proof.block_hash);
    assert_eq!(verified.block_number(), header.number);
    // The workflow consumer still needs exact body + signer + success, in
    // addition to these inclusion roots. This is not a completion certificate.
    crate::contract::qualification::verify_transaction_inclusion(
        expected.network,
        &header,
        &transaction.encoded_for_signing(),
        ACCOUNT,
        HASH,
        &proof,
    )?;
    Ok(())
}

#[test]
fn captured_proof_still_rejects_changed_binding_header_status_and_proof()
-> Result<(), Box<dyn std::error::Error>> {
    for mutation in 0..10 {
        let (mut expected, mut header, mut proof) = fixture()?;
        match mutation {
            0 => expected.network = EvmNetwork::EthereumMainnet,
            1 => expected.nonce += 1,
            2 => expected.account = Address::ZERO,
            3 => expected.signing_digest = B256::ZERO,
            4 => proof.transaction_proof.clear(),
            5 => proof.receipt_proof.clear(),
            6 => proof.transaction_index += 1,
            7 => header.gas_used += 1,
            8 => proof.receipt.replace_range(2..4, "01"),
            _ => {
                let bytes = alloy_primitives::hex::decode(&proof.receipt)?;
                let mut remaining = bytes.strip_prefix(&[2]).ok_or("not type2")?;
                let mut receipt = ReceiptWithBloom::<Receipt>::decode(&mut remaining)?;
                receipt.receipt.status = Eip658Value::Eip658(false);
                let mut changed = vec![2];
                changed.extend(alloy_rlp::encode(&receipt));
                proof.receipt = format!("0x{}", alloy_primitives::hex::encode(changed));
            }
        }
        assert!(
            expected.verify(&header, &proof).is_err(),
            "mutation={mutation}"
        );
    }
    Ok(())
}
