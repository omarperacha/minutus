//! Pure synthetic inclusion tests cannot mint a production certificate externally.
use super::*;
use alloy_consensus::{Eip658Value, Receipt, Signed, TxEip1559};
use alloy_primitives::{Signature, TxKind, U256};
use alloy_trie::{Nibbles, nodes::LeafNode};

fn trie(index: u64, bytes: &[u8]) -> (B256, Vec<String>) {
    let node = alloy_rlp::encode(LeafNode::new(
        Nibbles::unpack(alloy_rlp::encode(index)),
        bytes.to_vec(),
    ));
    (
        keccak256(&node),
        vec![format!("0x{}", alloy_primitives::hex::encode(node))],
    )
}

fn fixture(
    network: EvmNetwork,
    index: u64,
    succeeded: bool,
) -> Result<(Expected, Header, DeploymentProof), QualificationError> {
    let tx = TxEip1559 {
        chain_id: network.descriptor().chain_id(),
        nonce: 3,
        gas_limit: 100_000,
        max_fee_per_gas: 10,
        max_priority_fee_per_gas: 1,
        to: TxKind::Call(Address::repeat_byte(2)),
        ..Default::default()
    };
    let signing_digest = keccak256(tx.encoded_for_signing());
    let signature = Signature::from_scalars_and_parity(
        "0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
            .parse()
            .map_err(|_| QualificationError::InvalidIntent)?,
        B256::from(U256::from(1).to_be_bytes::<32>()),
        false,
    );
    let signed = Signed::new_unhashed(tx, signature);
    let account = signed
        .recover_signer()
        .map_err(|_| QualificationError::InvalidIntent)?;
    let mut transaction = Vec::new();
    signed.eip2718_encode(&mut transaction);
    let hash = keccak256(&transaction);
    let mut receipt = vec![2];
    receipt.extend(alloy_rlp::encode(
        Receipt {
            status: Eip658Value::Eip658(succeeded),
            cumulative_gas_used: 50_000,
            logs: Vec::<alloy_primitives::Log>::new(),
        }
        .with_bloom(),
    ));
    let (transactions_root, transaction_proof) = trie(index, &transaction);
    let (receipts_root, receipt_proof) = trie(index, &receipt);
    let header = Header {
        transactions_root,
        receipts_root,
        number: 100,
        gas_used: 50_000,
        ..Default::default()
    };
    let proof = DeploymentProof {
        block_hash: format!("{:#x}", header.hash_slow()),
        transaction_index: index,
        transaction: format!("0x{}", alloy_primitives::hex::encode(transaction)),
        transaction_proof,
        receipt: format!("0x{}", alloy_primitives::hex::encode(receipt)),
        receipt_proof,
    };
    Ok((
        Expected {
            network,
            hash,
            account,
            nonce: 3,
            signing_digest,
        },
        header,
        proof,
    ))
}

#[test]
fn exact_receipt_certificate_accepts_success_or_revert_on_all_networks()
-> Result<(), QualificationError> {
    for network in EvmNetwork::ALL {
        for succeeded in [false, true] {
            for index in [0, 127, 128, 65_536] {
                let (expected, header, proof) = fixture(network, index, succeeded)?;
                let verified = expected.verify(&header, &proof)?;
                assert_eq!(verified.network(), network);
                assert_eq!(verified.account(), format!("{:#x}", expected.account));
                assert_eq!(verified.nonce(), "3");
                assert_eq!(verified.transaction_hash(), format!("{:#x}", expected.hash));
                assert_eq!(
                    verified.signing_digest(),
                    format!("{:#x}", expected.signing_digest)
                );
                assert_eq!(verified.block_number(), header.number);
                assert_eq!(verified.block_hash(), proof.block_hash);
                assert_eq!(verified.succeeded(), succeeded);
                assert_eq!(verified.inclusion_commitment().len(), 66);
            }
        }
    }
    Ok(())
}

#[test]
fn all_persisted_bindings_and_both_mpt_proofs_are_required() -> Result<(), QualificationError> {
    for mutation in 0..12 {
        let (mut expected, header, mut proof) = fixture(EvmNetwork::EthereumMainnet, 128, true)?;
        match mutation {
            0 => expected.network = EvmNetwork::BaseMainnet,
            1 => expected.hash = B256::ZERO,
            2 => expected.account = Address::ZERO,
            3 => expected.nonce += 1,
            4 => expected.signing_digest = B256::ZERO,
            5 => proof.transaction_proof.clear(),
            6 => proof.receipt_proof.clear(),
            7 => proof.transaction_index = 127,
            8 => proof.block_hash = format!("{:#x}", B256::ZERO),
            9 => proof.transaction.push_str("00"),
            10 => proof.receipt.push_str("00"),
            _ => proof.receipt.replace_range(2..4, "01"),
        }
        assert!(
            expected.verify(&header, &proof).is_err(),
            "mutation={mutation}"
        );
    }
    Ok(())
}

#[test]
fn canonical_expected_facts_reject_before_acquisition() -> Result<(), QualificationError> {
    let (expected, _, _) = fixture(EvmNetwork::EthereumMainnet, 0, true)?;
    let hash = format!("{:#x}", expected.hash);
    let account = format!("{:#x}", expected.account);
    let digest = format!("{:#x}", expected.signing_digest);
    Expected::parse(expected.network, &hash, &account, "3", &digest)?;
    for nonce in ["03", "0x3", "-1", "18446744073709551616"] {
        assert!(Expected::parse(expected.network, &hash, &account, nonce, &digest).is_err());
    }
    for invalid in ["", "0x12", "not-a-hash"] {
        assert!(Expected::parse(expected.network, invalid, &account, "3", &digest).is_err());
        assert!(Expected::parse(expected.network, &hash, &account, "3", invalid).is_err());
    }
    Ok(())
}
