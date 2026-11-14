//! Debug-only read-only qualification before all application/profile setup.
//! No vault, native approval, signing or transaction transport is in this route.
#[cfg(debug_assertions)]
pub(crate) fn run(arguments: &[std::ffi::OsString]) -> Result<(), Box<dyn std::error::Error>> {
    use chain_evm::{
        EvmNetwork, EvmRpcUrl,
        contract::{
            ContractBlock,
            generic::{
                GenericSourceProvider, generic_contract_intent, prepare_generic_contract,
                qualify_generic_source_at,
            },
        },
        local_simulation::contract_fee_facts,
    };
    use evm_simulation::TrustedSimulationWorkers;
    use std::sync::atomic::AtomicBool;
    if !(5..=6).contains(&arguments.len()) {
        return Err(
            "expected network target function argumentsJSON publicCaller [canonicalBlockHash]"
                .into(),
        );
    }
    let text = arguments
        .iter()
        .map(|arg| arg.to_str().ok_or("non-UTF8 qualification argument"))
        .collect::<Result<Vec<_>, _>>()?;
    let network = match text[0] {
        "ethereumSepolia" => EvmNetwork::EthereumSepolia,
        "baseSepolia" => EvmNetwork::BaseSepolia,
        "ethereumMainnet" => EvmNetwork::EthereumMainnet,
        "baseMainnet" => EvmNetwork::BaseMainnet,
        _ => return Err("unsupported qualification network".into()),
    };
    let arguments: Vec<serde_json::Value> = serde_json::from_str(text[3])?;
    let block = text.get(5).map_or(ContractBlock::Latest, |hash| {
        ContractBlock::Hash((*hash).into())
    });
    let endpoint = EvmRpcUrl::new(network.descriptor().default_rpc_url().into())?;
    let cancellation = AtomicBool::new(false);
    // A fresh public-source cache is unrelated to every app profile/vault.
    let directory =
        std::env::temp_dir().join(format!("minutus-contract-source-{}", uuid::Uuid::now_v7()));
    let mut sources = GenericSourceProvider::packaged(&directory)?;
    let identity = qualify_generic_source_at(
        network,
        &endpoint,
        text[1],
        &block,
        &mut sources,
        &cancellation,
    )
    .map_err(|error| format!("source correspondence: {error}"))?;
    let intent = generic_contract_intent(&identity, text[4], "0", text[2], &arguments)?;
    let workers = TrustedSimulationWorkers::packaged()
        .map_err(|error| format!("packaged execution helpers: {error}"))?;
    let fees = contract_fee_facts(network, &endpoint, &cancellation)
        .map_err(|error| format!("public snapshot fee facts: {error}"))?;
    let candidate = prepare_generic_contract(
        &workers,
        &endpoint,
        &intent,
        &block,
        fees,
        &mut sources,
        &cancellation,
    )
    .map_err(|error| format!("proved transition and envelope: {error}"))?;
    println!(
        "{}",
        serde_json::json!({"mode":"unsignedReadOnlyQualification",
        "noProfileVaultSigningOrSend":true,"network":network.descriptor().caip2_id(),
        "requestedBlockHash":text.get(5),"currentTipGuaranteed":false,
        "publicCaller":intent.account(),"accountControlProved":false,
        "target":intent.call().target(),"function":intent.call().signature(),"arguments":intent.call().arguments(),
        "blockHash":candidate.block_hash(),"blockNumber":candidate.block_number().to_string(),
        "sources":candidate.identity().contracts(),"sourceCommitment":candidate.source_commitment(),
        "effectEnvelope":candidate.envelope().relations(),"storage":candidate.effects().storage_changes(),
        "output":candidate.effects().output(),"logs":candidate.effects().logs(),
        "maximumSnapshotFeeWei":candidate.maximum_fee_wei(),"candidate":candidate.candidate_digest()})
    );
    Ok(())
}
