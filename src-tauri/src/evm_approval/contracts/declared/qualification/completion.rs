//! Fixed public historical transaction and current source-condition diagnosis.
//! No application profile, wallet, execution claim, candidate or signer is opened.
use chain_evm::{
    EvmNetwork, EvmRpcUrl,
    contract::{
        acquire_contract_transaction_proof,
        generic::{
            GenericComparison, GenericCondition, GenericSourceProvider, admit_generic_condition,
            diagnose_generic_receipt_conditions, qualify_generic_source,
        },
    },
};
use ipc_types::IpcError;
use serde_json::json;
use std::{path::Path, sync::atomic::AtomicBool, time::Instant};

const HASH: &str = "0x7080f780fc5bada96b17756bf702b191b8b62994afa606a6c37bfb2da4aa5b0c";
const TARGET: &str = "0x2738d13e81e30bc615766a0410e7cf199fd59a83";

pub(super) fn run(
    endpoint: &EvmRpcUrl,
    cache: &Path,
    cancelled: &AtomicBool,
) -> Result<String, IpcError> {
    let network = EvmNetwork::EthereumSepolia;
    let started = Instant::now();
    eprintln!("Completion diagnostic stage: ExistingPublicInclusion");
    let inclusion = acquire_contract_transaction_proof(network, endpoint, HASH, cancelled)
        .map_err(|error| {
            IpcError::invalid_request(format!("Existing public inclusion: {error}"))
        })?;
    let inclusion_ms = started.elapsed().as_millis();
    eprintln!("Completion diagnostic stage: ReproducedSource");
    let started = Instant::now();
    let mut sources = GenericSourceProvider::packaged(cache)
        .map_err(|error| IpcError::invalid_request(format!("Packaged compiler: {error}")))?;
    let identity = qualify_generic_source(network, endpoint, TARGET, &mut sources, cancelled)
        .map_err(|error| IpcError::invalid_request(format!("Reproduced source: {error}")))?;
    let source_ms = started.elapsed().as_millis();
    let condition = GenericCondition {
        target: TARGET.into(),
        path: vec!["number".into()],
        comparison: GenericComparison::Equal,
        expected: json!("42"),
    };
    admit_generic_condition(&identity, &condition)
        .map_err(|error| IpcError::invalid_request(format!("Condition declaration: {error}")))?;
    eprintln!("Completion diagnostic stage: CurrentSourceCondition");
    let started = Instant::now();
    let observed = diagnose_generic_receipt_conditions(
        endpoint,
        network,
        &inclusion,
        &[&identity],
        &[condition],
        cancelled,
    )
    .map_err(|error| {
        IpcError::invalid_request(format!("Post-inclusion source condition: {error}"))
    })?;
    Ok(json!({
        "mode":"existingPublicTransactionAndCurrentConditionDiagnosis",
        "transactionHash":HASH,"target":TARGET,"network":network.descriptor().caip2_id(),
        "publicInclusion":{"blockHash":inclusion.block_hash,"elapsedMs":inclusion_ms,
            "qualification":"canonical header and complete transaction/receipt trie roots; original approved-body binding is not reloaded by this diagnostic"},
        "sourceElapsedMs":source_ms,"conditionElapsedMs":started.elapsed().as_millis(),
        "currentCondition":{"blockHash":observed.block_hash(),"blockNumber":observed.block_number().to_string(),"observations":observed.observations()},
        "historicalWorkflowCompletion":"notReverified","writeAuthorityGranted":false,
        "effectsQualified":false,"accountControlProved":false,"noProfileVaultSigningOrSend":true
    }).to_string())
}
