//! Exact creation provenance preserves the reviewed token's initial supply invariant.
//! The caller owns historical-header canonicality; this module only checks inclusion.
use super::QualificationError;
use crate::EvmNetwork;
use alloy_consensus::{Eip658Value, Header, Receipt, ReceiptWithBloom, Signed, TxEip1559};
use alloy_dyn_abi::{DynSolType, DynSolValue};
use alloy_primitives::{Address, B256, Bytes, TxKind, U256, keccak256};
use alloy_rlp::Decodable;
use alloy_trie::{Nibbles, proof::verify_proof};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

const CREATION: &str = include_str!("../../../profiles/erc20-oz-5.7.0/creation.hex");
const CREATION_SHA256: &str = "f6dfdcc0707672994d32c84a907460f20f69532878d0e77c8f7ceba31bbc6ba5";
const MAX_TRANSACTION_BYTES: usize = 8192;
const MAX_RECEIPT_BYTES: usize = 4096;
const MAX_PROOF_NODES: usize = 64;
const MAX_PROOF_BYTES: usize = 128 * 1024;
const MAX_LABEL_BYTES: usize = 1024;

/// Untrusted historical evidence. It grants no source or execution authority.
/// RPC/IPC owners must also bound their complete response before deserialisation.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentProof {
    /// Hash of the historical block, independently fetched by the host.
    pub block_hash: String,
    /// Transaction and receipt trie position within that block.
    pub transaction_index: u64,
    /// Exact signed EIP-2718 transaction hex, excluded from diagnostics.
    pub transaction: String,
    /// Ordered transaction-trie inclusion nodes as hex.
    pub transaction_proof: Vec<String>,
    /// Exact EIP-2718 receipt hex, excluded from diagnostics.
    pub receipt: String,
    /// Ordered receipt-trie inclusion nodes as hex.
    pub receipt_proof: Vec<String>,
}

impl fmt::Debug for DeploymentProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeploymentProof")
            .finish_non_exhaustive()
    }
}

/// Certificate minted only after exact creation and both inclusion checks.
/// It contains no signed transaction, arbitrary source text or token labels.
pub(super) struct VerifiedDeployment {
    network: EvmNetwork,
    block_hash: B256,
    block_number: u64,
    target: Address,
    transaction_hash: B256,
}

impl VerifiedDeployment {
    #[cfg(test)]
    pub(super) fn test_fixture() -> Result<(Self, Address), QualificationError> {
        let (header, target, proof) = tests::fixture(0, U256::from(17))?;
        let verified =
            Self::verify_against_header(&header, EvmNetwork::EthereumSepolia, target, &proof)?;
        Ok((verified, target))
    }
    pub(super) fn verify_against_header(
        header: &Header,
        network: EvmNetwork,
        target: Address,
        evidence: &DeploymentProof,
    ) -> Result<Self, QualificationError> {
        let reject = QualificationError::InvalidDeployment;
        let block_hash = B256::from_slice(&hex(&evidence.block_hash, 32, true)?);
        if header.hash_slow() != block_hash || target.is_zero() {
            return Err(reject);
        }
        let transaction = hex(&evidence.transaction, MAX_TRANSACTION_BYTES, false)?;
        let receipt = hex(&evidence.receipt, MAX_RECEIPT_BYTES, false)?;
        let transaction_proof = proof_nodes(&evidence.transaction_proof)?;
        let receipt_proof = proof_nodes(&evidence.receipt_proof)?;
        let mut remaining = transaction.as_slice();
        let signed = Signed::<TxEip1559>::eip2718_decode(&mut remaining).map_err(|_| reject)?;
        let mut canonical = Vec::new();
        signed.eip2718_encode(&mut canonical);
        let tx = signed.tx();
        if !remaining.is_empty()
            || transaction.first() != Some(&2)
            || canonical != transaction
            || tx.chain_id != network.descriptor().chain_id()
            || tx.to != TxKind::Create
            || !tx.value.is_zero()
            || !tx.access_list.is_empty()
            || tx.nonce == u64::MAX
            || tx.gas_limit == 0
            || tx.gas_limit > header.gas_limit
            || tx.max_priority_fee_per_gas > tx.max_fee_per_gas
            || signed.signature().normalize_s().is_some()
        {
            return Err(reject);
        }
        let creator = signed.recover_signer().map_err(|_| reject)?;
        if creator.create(tx.nonce) != target {
            return Err(reject);
        }
        let (holder, supply) = constructor(&tx.input)?;
        successful_receipt(&receipt, header, target, holder, supply)?;
        // Transaction/receipt tries use the nibbles of RLP(index), not a hash.
        let key = Nibbles::unpack(alloy_rlp::encode(evidence.transaction_index));
        verify_proof(
            header.transactions_root,
            key,
            Some(transaction.clone()),
            &transaction_proof,
        )
        .map_err(|_| reject)?;
        verify_proof(header.receipts_root, key, Some(receipt), &receipt_proof)
            .map_err(|_| reject)?;
        Ok(Self {
            network,
            block_hash,
            block_number: header.number,
            target,
            transaction_hash: keccak256(transaction),
        })
    }

    pub(super) const fn network(&self) -> EvmNetwork {
        self.network
    }
    pub(super) const fn block_hash(&self) -> B256 {
        self.block_hash
    }
    pub(super) const fn block_number(&self) -> u64 {
        self.block_number
    }
    pub(super) const fn target(&self) -> Address {
        self.target
    }
    pub(super) const fn transaction_hash(&self) -> B256 {
        self.transaction_hash
    }
}

fn constructor(input: &[u8]) -> Result<(Address, U256), QualificationError> {
    let reject = QualificationError::InvalidDeployment;
    if format!("{:x}", Sha256::digest(CREATION.as_bytes())) != CREATION_SHA256 {
        return Err(QualificationError::InvalidArtifact);
    }
    let creation = hex(CREATION.trim(), MAX_TRANSACTION_BYTES, false)?;
    let args = input.strip_prefix(creation.as_slice()).ok_or(reject)?;
    // Bound dynamic lengths before the maintained ABI decoder allocates strings.
    if args.len() < 192 || args.len() > 192 + 2 * MAX_LABEL_BYTES {
        return Err(reject);
    }
    for offset_word in [&args[..32], &args[32..64]] {
        let offset = bounded_word(offset_word, args.len())?;
        let end = offset.checked_add(32).ok_or(reject)?;
        let length = bounded_word(args.get(offset..end).ok_or(reject)?, MAX_LABEL_BYTES)?;
        if end.checked_add(length).is_none_or(|end| end > args.len()) {
            return Err(reject);
        }
    }
    let schema = DynSolType::Tuple(vec![
        DynSolType::String,
        DynSolType::String,
        DynSolType::Address,
        DynSolType::Uint(256),
    ]);
    let value = schema.abi_decode_params(args).map_err(|_| reject)?;
    if value.abi_encode_params() != args {
        return Err(reject);
    }
    let DynSolValue::Tuple(values) = value else {
        return Err(reject);
    };
    match values.as_slice() {
        [
            DynSolValue::String(name),
            DynSolValue::String(symbol),
            DynSolValue::Address(holder),
            DynSolValue::Uint(supply, 256),
        ] if name.len() <= MAX_LABEL_BYTES
            && symbol.len() <= MAX_LABEL_BYTES
            && !holder.is_zero() =>
        {
            Ok((*holder, *supply))
        }
        _ => Err(reject),
    }
}

fn bounded_word(word: &[u8], maximum: usize) -> Result<usize, QualificationError> {
    let value: usize = U256::from_be_slice(word)
        .try_into()
        .map_err(|_| QualificationError::InvalidDeployment)?;
    if value > maximum {
        return Err(QualificationError::InvalidDeployment);
    }
    Ok(value)
}

fn successful_receipt(
    bytes: &[u8],
    header: &Header,
    target: Address,
    holder: Address,
    supply: U256,
) -> Result<(), QualificationError> {
    let reject = QualificationError::InvalidDeployment;
    let mut remaining = bytes.strip_prefix(&[2]).ok_or(reject)?;
    let receipt = ReceiptWithBloom::<Receipt>::decode(&mut remaining).map_err(|_| reject)?;
    let mut canonical = vec![2];
    canonical.extend(alloy_rlp::encode(&receipt));
    if !remaining.is_empty()
        || canonical != bytes
        || receipt.receipt.status != Eip658Value::Eip658(true)
        || receipt.receipt.cumulative_gas_used == 0
        || receipt.receipt.cumulative_gas_used > header.gas_used
        || receipt.logs_bloom != receipt.receipt.bloom_slow()
    {
        return Err(reject);
    }
    let [log] = receipt.receipt.logs.as_slice() else {
        return Err(reject);
    };
    let holder_topic = B256::from(U256::from_be_slice(holder.as_slice()).to_be_bytes::<32>());
    if log.address != target
        || log.data.topics()
            != [
                keccak256("Transfer(address,address,uint256)"),
                B256::ZERO,
                holder_topic,
            ]
        || log.data.data.as_ref() != supply.to_be_bytes::<32>()
    {
        return Err(reject);
    }
    Ok(())
}

pub(super) fn proof_nodes(values: &[String]) -> Result<Vec<Bytes>, QualificationError> {
    if values.is_empty() || values.len() > MAX_PROOF_NODES {
        return Err(QualificationError::InvalidDeployment);
    }
    let mut total = 0_usize;
    values
        .iter()
        .map(|value| {
            total = total
                .checked_add(value.len())
                .ok_or(QualificationError::InvalidDeployment)?;
            if total > MAX_PROOF_BYTES * 2 {
                return Err(QualificationError::InvalidDeployment);
            }
            hex(value, MAX_PROOF_BYTES, false).map(Bytes::from)
        })
        .collect()
}

pub(super) fn hex(value: &str, maximum: usize, exact: bool) -> Result<Vec<u8>, QualificationError> {
    let reject = QualificationError::InvalidDeployment;
    let digits = value.strip_prefix("0x").ok_or(reject)?;
    if digits.is_empty()
        || digits.len() % 2 != 0
        || digits.len() > maximum * 2
        || (exact && digits.len() != maximum * 2)
    {
        return Err(reject);
    }
    digits
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            Ok(digit(pair[0]).ok_or(reject)? * 16 + digit(pair[1]).ok_or(reject)?)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Log, LogData, Signature};
    use alloy_trie::nodes::LeafNode;

    fn constructor_input(holder: Address, supply: U256) -> Result<Bytes, QualificationError> {
        let mut input = hex(CREATION.trim(), MAX_TRANSACTION_BYTES, false)?;
        input.extend(
            DynSolValue::Tuple(vec![
                DynSolValue::String(String::new()),
                DynSolValue::String("Untrusted label".into()),
                DynSolValue::Address(holder),
                DynSolValue::Uint(supply, 256),
            ])
            .abi_encode_params(),
        );
        Ok(input.into())
    }

    pub(super) fn fixture(
        index: u64,
        supply: U256,
    ) -> Result<(Header, Address, DeploymentProof), QualificationError> {
        fixture_with_transaction(index, supply, |_| {})
    }

    fn fixture_with_transaction(
        index: u64,
        supply: U256,
        mutate: impl FnOnce(&mut TxEip1559),
    ) -> Result<(Header, Address, DeploymentProof), QualificationError> {
        let holder = Address::repeat_byte(0x42);
        let mut tx = TxEip1559 {
            chain_id: EvmNetwork::EthereumSepolia.descriptor().chain_id(),
            nonce: 3,
            gas_limit: 2_000_000,
            max_fee_per_gas: 10,
            max_priority_fee_per_gas: 1,
            to: TxKind::Create,
            input: constructor_input(holder, supply)?,
            ..Default::default()
        };
        mutate(&mut tx);
        // Public synthetic recoverable scalars: no signing key or signer API.
        // R is the secp256k1 generator's x-coordinate and s is one (low-s).
        let r = B256::from_slice(&hex(
            "0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
            32,
            true,
        )?);
        let signature = Signature::from_scalars_and_parity(
            r,
            B256::from(U256::from(1).to_be_bytes::<32>()),
            false,
        );
        let signed = Signed::new_unhashed(tx, signature);
        let target = signed
            .recover_signer()
            .map_err(|_| QualificationError::InvalidDeployment)?
            .create(signed.tx().nonce);
        let mut transaction = Vec::new();
        signed.eip2718_encode(&mut transaction);
        let log = Log {
            address: target,
            data: LogData::new_unchecked(
                vec![
                    keccak256("Transfer(address,address,uint256)"),
                    B256::ZERO,
                    B256::from(U256::from_be_slice(holder.as_slice()).to_be_bytes::<32>()),
                ],
                Bytes::copy_from_slice(&supply.to_be_bytes::<32>()),
            ),
        };
        let mut receipt = vec![2];
        receipt.extend(alloy_rlp::encode(
            Receipt {
                status: Eip658Value::Eip658(true),
                cumulative_gas_used: 1_000_000,
                logs: vec![log],
            }
            .with_bloom(),
        ));
        let (transactions_root, transaction_proof) = inclusion(index, &transaction);
        let (receipts_root, receipt_proof) = inclusion(index, &receipt);
        let header = Header {
            transactions_root,
            receipts_root,
            number: 42,
            gas_limit: 30_000_000,
            gas_used: 1_000_000,
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
        Ok((header, target, proof))
    }

    fn inclusion(index: u64, bytes: &[u8]) -> (B256, Vec<String>) {
        let node = alloy_rlp::encode(LeafNode::new(
            Nibbles::unpack(alloy_rlp::encode(index)),
            bytes.to_vec(),
        ));
        (
            keccak256(&node),
            vec![format!("0x{}", alloy_primitives::hex::encode(node))],
        )
    }

    #[test]
    fn exact_create_and_receipt_inclusions_qualify_all_index_and_supply_boundaries()
    -> Result<(), QualificationError> {
        for index in [0, 1, 127, 128, 65_536] {
            for supply in [U256::ZERO, U256::from(17), U256::MAX] {
                let (header, target, proof) = fixture(index, supply)?;
                let verified = VerifiedDeployment::verify_against_header(
                    &header,
                    EvmNetwork::EthereumSepolia,
                    target,
                    &proof,
                )?;
                assert_eq!(verified.block_hash(), header.hash_slow());
                assert_eq!(verified.block_number(), 42);
                assert_eq!(verified.target(), target);
                assert_eq!(
                    verified.transaction_hash(),
                    keccak256(hex(&proof.transaction, MAX_TRANSACTION_BYTES, false)?)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn inclusion_identity_and_all_untrusted_payload_mutations_reject()
    -> Result<(), QualificationError> {
        let (header, target, proof) = fixture(0, U256::from(17))?;
        assert!(
            VerifiedDeployment::verify_against_header(
                &header,
                EvmNetwork::EthereumMainnet,
                target,
                &proof
            )
            .is_err()
        );
        assert!(
            VerifiedDeployment::verify_against_header(
                &header,
                EvmNetwork::EthereumSepolia,
                Address::repeat_byte(7),
                &proof
            )
            .is_err()
        );
        for mutation in 0..9 {
            let mut changed = proof.clone();
            match mutation {
                0 => changed.block_hash = format!("{:#x}", B256::ZERO),
                1 => changed.transaction_index = 1,
                2 => changed.transaction.push_str("00"),
                3 => changed.receipt.push_str("00"),
                4 => changed.transaction_proof.clear(),
                5 => changed.receipt_proof.clear(),
                6 => changed
                    .transaction_proof
                    .push(changed.transaction_proof[0].clone()),
                7 => changed.receipt_proof = vec!["0x00".into()],
                _ => changed.transaction = "0x00".repeat(MAX_TRANSACTION_BYTES),
            }
            assert!(
                VerifiedDeployment::verify_against_header(
                    &header,
                    EvmNetwork::EthereumSepolia,
                    target,
                    &changed
                )
                .is_err(),
                "mutation {mutation}"
            );
        }
        assert!(!format!("{proof:?}").contains(&proof.transaction));
        Ok(())
    }

    #[test]
    fn canonical_constructor_bounds_and_source_are_required() -> Result<(), QualificationError> {
        let holder = Address::repeat_byte(9);
        let input = constructor_input(holder, U256::MAX)?;
        assert_eq!(constructor(&input)?, (holder, U256::MAX));
        let prefix_len = hex(CREATION.trim(), MAX_TRANSACTION_BYTES, false)?.len();
        for mutation in 0..6 {
            let mut changed = input.to_vec();
            match mutation {
                0 => changed[0] ^= 1,
                1 => changed.push(0),
                2 => changed[prefix_len + 31] = 0,
                3 => changed[prefix_len + 64..prefix_len + 96].fill(0),
                4 => changed[prefix_len..prefix_len + 32].fill(0xff),
                _ => changed[prefix_len + 128..prefix_len + 160].fill(0xff),
            }
            assert!(
                constructor(&changed).is_err(),
                "constructor mutation {mutation}"
            );
        }
        for size in [MAX_LABEL_BYTES, MAX_LABEL_BYTES + 1] {
            let mut labelled = hex(CREATION.trim(), MAX_TRANSACTION_BYTES, false)?;
            labelled.extend(
                DynSolValue::Tuple(vec![
                    DynSolValue::String("a".repeat(size)),
                    DynSolValue::String("é".repeat(size / 2)),
                    DynSolValue::Address(holder),
                    DynSolValue::Uint(U256::ZERO, 256),
                ])
                .abi_encode_params(),
            );
            assert_eq!(constructor(&labelled).is_ok(), size == MAX_LABEL_BYTES);
        }
        Ok(())
    }

    #[test]
    fn altered_transaction_bodies_reject_even_with_matching_inclusions()
    -> Result<(), QualificationError> {
        for mutation in 0..8 {
            let (header, target, proof) =
                fixture_with_transaction(0, U256::from(17), |tx| match mutation {
                    0 => tx.to = TxKind::Call(Address::repeat_byte(3)),
                    1 => tx.value = U256::from(1),
                    2 => tx.nonce = u64::MAX,
                    3 => tx.gas_limit = 30_000_001,
                    4 => tx.max_priority_fee_per_gas = 11,
                    5 => tx.input = Bytes::from(vec![0; tx.input.len()]),
                    6 => tx.input = Bytes::from([tx.input.as_ref(), &[0]].concat()),
                    _ => tx.access_list.0.resize_with(1, Default::default),
                })?;
            assert!(
                VerifiedDeployment::verify_against_header(
                    &header,
                    EvmNetwork::EthereumSepolia,
                    target,
                    &proof
                )
                .is_err(),
                "transaction mutation {mutation}"
            );
        }
        Ok(())
    }

    #[test]
    fn high_s_signatures_and_hashed_index_paths_cannot_qualify() -> Result<(), QualificationError> {
        let (mut header, target, mut proof) = fixture(0, U256::from(17))?;
        let transaction = hex(&proof.transaction, MAX_TRANSACTION_BYTES, false)?;
        let signed = Signed::<TxEip1559>::eip2718_decode(&mut transaction.as_slice())
            .map_err(|_| QualificationError::InvalidDeployment)?;
        let high_s = B256::from_slice(&hex(
            "0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140",
            32,
            true,
        )?);
        let signature = Signature::from_scalars_and_parity(
            B256::from(signed.signature().r().to_be_bytes::<32>()),
            high_s,
            true,
        );
        let changed = Signed::new_unhashed(signed.strip_signature(), signature);
        assert_eq!(
            changed
                .recover_signer()
                .map_err(|_| QualificationError::InvalidDeployment)?
                .create(changed.tx().nonce),
            target
        );
        let mut changed_bytes = Vec::new();
        changed.eip2718_encode(&mut changed_bytes);
        (header.transactions_root, proof.transaction_proof) = inclusion(0, &changed_bytes);
        proof.transaction = format!("0x{}", alloy_primitives::hex::encode(changed_bytes));
        proof.block_hash = format!("{:#x}", header.hash_slow());
        assert!(
            VerifiedDeployment::verify_against_header(
                &header,
                EvmNetwork::EthereumSepolia,
                target,
                &proof
            )
            .is_err()
        );

        let (mut header, target, mut proof) = fixture(0, U256::from(17))?;
        let node = alloy_rlp::encode(LeafNode::new(
            Nibbles::unpack(keccak256(alloy_rlp::encode(0_u64))),
            hex(&proof.transaction, MAX_TRANSACTION_BYTES, false)?,
        ));
        header.transactions_root = keccak256(&node);
        proof.transaction_proof = vec![format!("0x{}", alloy_primitives::hex::encode(node))];
        proof.block_hash = format!("{:#x}", header.hash_slow());
        assert!(
            VerifiedDeployment::verify_against_header(
                &header,
                EvmNetwork::EthereumSepolia,
                target,
                &proof
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn failed_or_spoofed_mint_receipts_reject_even_if_their_roots_match()
    -> Result<(), QualificationError> {
        let (mut header, target, mut proof) = fixture(0, U256::from(17))?;
        let receipt = hex(&proof.receipt, MAX_RECEIPT_BYTES, false)?;
        let original = ReceiptWithBloom::<Receipt>::decode(&mut &receipt[1..])
            .map_err(|_| QualificationError::InvalidDeployment)?;
        for mutation in 0..5 {
            let mut changed = original.clone();
            match mutation {
                0 => changed.receipt.status = Eip658Value::Eip658(false),
                1 => changed.receipt.logs[0].address = Address::ZERO,
                2 => changed.receipt.logs[0].data.data = Bytes::from(vec![0; 32]),
                3 => changed.receipt.logs.clear(),
                _ => changed.receipt.logs.push(changed.receipt.logs[0].clone()),
            }
            changed.logs_bloom = changed.receipt.bloom_slow();
            let mut bytes = vec![2];
            bytes.extend(alloy_rlp::encode(changed));
            (header.receipts_root, proof.receipt_proof) = inclusion(0, &bytes);
            proof.receipt = format!("0x{}", alloy_primitives::hex::encode(bytes));
            proof.block_hash = format!("{:#x}", header.hash_slow());
            assert!(
                VerifiedDeployment::verify_against_header(
                    &header,
                    EvmNetwork::EthereumSepolia,
                    target,
                    &proof
                )
                .is_err()
            );
        }
        Ok(())
    }
}
