//! Explicit ignored public-read qualification. No wallet, account selection,
//! private configuration, signature, source upload or submission is involved.
use super::*;

fn existing_transaction(
    network: EvmNetwork,
    endpoint: &EvmRpcUrl,
    cancelled: &AtomicBool,
) -> Result<(String, bool), QualificationError> {
    let control = SimulationControl::start(cancelled);
    let mut reader = Acquisition {
        rpc: RpcClient::new(endpoint).map_err(|_| SimulationError::StateUnavailable)?,
        control: &control,
        bytes: 0,
    };
    let chain = reader.call("eth_chainId", &json!([]))?;
    network
        .validate_rpc_chain_id(chain.as_str().ok_or(QualificationError::SnapshotMismatch)?)
        .map_err(|_| QualificationError::SnapshotMismatch)?;
    let mut block = reader.call("eth_getBlockByNumber", &json!(["latest", true]))?;
    for _ in 0..8 {
        let entries = block
            .get("transactions")
            .and_then(Value::as_array)
            .ok_or(QualificationError::SnapshotMismatch)?;
        let mut candidates = entries
            .iter()
            .filter(|transaction| {
                transaction["type"] == "0x2"
                    && transaction["input"]
                        .as_str()
                        .is_some_and(|input| input.len() <= 2050)
            })
            .collect::<Vec<_>>();
        candidates
            .sort_by_key(|transaction| transaction["input"].as_str().map_or(usize::MAX, str::len));
        if let Some(transaction) = candidates.first() {
            let selected = format!("{:#x}", hash(transaction, "hash")?);
            let block_hash = hash(&block, "hash")?;
            let batch =
                reader.optional("eth_getBlockReceipts", &json!([format!("{block_hash:#x}")]))?;
            let supported = match batch {
                Some(value) if value.is_array() => true,
                None => false,
                Some(_) => return Err(QualificationError::SnapshotMismatch),
            };
            return Ok((selected, supported));
        }
        let number = quantity(&block, "number")?
            .checked_sub(1)
            .ok_or(QualificationError::SnapshotMismatch)?;
        block = reader.call(
            "eth_getBlockByNumber",
            &json!([format!("0x{number:x}"), true]),
        )?;
    }
    Err(SimulationError::StateUnavailable.into())
}

#[test]
#[ignore = "explicit read-only default-provider receipt acquisition qualification; no CI network dependency"]
fn default_providers_prove_existing_receipts_with_standard_block_methods()
-> Result<(), Box<dyn std::error::Error>> {
    let mut failures = Vec::new();
    for network in EvmNetwork::ALL {
        let endpoint = EvmRpcUrl::new(network.descriptor().default_rpc_url().into())?;
        let cancelled = AtomicBool::new(false);
        let result=existing_transaction(network,&endpoint,&cancelled).and_then(|(hash,batch_supported)|{
            let proof=acquire_contract_transaction_proof(network,&endpoint,&hash,&cancelled)?;
            Ok(json!({"network":network.descriptor().caip2_id(),"transactionHash":hash,"blockHash":proof.block_hash,"transactionIndex":proof.transaction_index,"transactionProofNodes":proof.transaction_proof.len(),"receiptProofNodes":proof.receipt_proof.len(),"status":"both complete trie roots verified","batchReceiptRpcSupported":batch_supported,"methods":"standard full-block JSON; batch-first receipt acquisition with unsupported-only individual fallback; no raw-transaction method"}))
        });
        match result {
            Ok(facts) => println!("{facts}"),
            Err(error) => {
                let message = format!("{}: {error}", network.descriptor().caip2_id());
                println!(
                    "{}",
                    json!({"network":network.descriptor().caip2_id(),"status":"unverified","error":error.to_string()})
                );
                failures.push(message);
            }
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; ").into());
    }
    Ok(())
}

#[test]
#[ignore = "explicit read-only qualification using public transactions observed on2026-11-10; no CI network dependency"]
fn default_providers_prove_known_public_receipts_without_live_block_selection()
-> Result<(), Box<dyn std::error::Error>> {
    // Stable public samples avoid conflating an empty recent testnet window
    // with provider support. Every included body/root/canonicality check remains
    // the production path; these hashes confer no signing or source authority.
    let samples = [
        (
            EvmNetwork::BaseMainnet,
            "0x472665e3a217a1299bf2df987485962fea38c6cb04dd07096a6e02298d57d114",
        ),
        (
            EvmNetwork::BaseSepolia,
            "0x40006ebc170bcbf82c77179f1ee800fa9f014321c8e8373dcbf992dca89c5dba",
        ),
        (
            EvmNetwork::EthereumMainnet,
            "0x71bfc20f8615ff1dfabe8d48be9e06eaa3c4d08def7158081fca75983848d465",
        ),
        (
            EvmNetwork::EthereumSepolia,
            "0xb1010586edc435a510f46226d5a0c3a7c1801e6bbd4bf74842eb71251738edea",
        ),
    ];
    let mut failures = Vec::new();
    for (network, hash) in samples {
        let endpoint = EvmRpcUrl::new(network.descriptor().default_rpc_url().into())?;
        let cancelled = AtomicBool::new(false);
        match acquire_contract_transaction_proof(network, &endpoint, hash, &cancelled) {
            Ok(proof) => println!(
                "{}",
                json!({"network":network.descriptor().caip2_id(),"transactionHash":hash,"blockHash":proof.block_hash,"transactionIndex":proof.transaction_index,"status":"both complete trie roots and canonicality verified","selection":"fixed existing public sample; no dependency on recent testnet activity","rawTransactionRpc":false})
            ),
            Err(error) => {
                println!(
                    "{}",
                    json!({"network":network.descriptor().caip2_id(),"status":"unverified","error":error.to_string()})
                );
                failures.push(format!("{}: {error}", network.descriptor().caip2_id()));
            }
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; ").into());
    }
    Ok(())
}
