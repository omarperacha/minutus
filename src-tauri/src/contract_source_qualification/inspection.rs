//! Debug-only source inspection using native RPC settings before app/vault setup.
use chain_evm::{
    EvmNetwork, EvmRpcUrl, PublicEvmAddress,
    contract::{
        ContractBlock,
        generic::{GenericError, GenericSourceProvider, inspect_generic_source},
        observe_contract_code_with_admission,
    },
};
use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString},
    path::Path,
    sync::atomic::AtomicBool,
    time::Instant,
};

struct Request {
    network: EvmNetwork,
    target: PublicEvmAddress,
    block: ContractBlock,
    repeat: bool,
}
impl Request {
    fn parse(arguments: &[OsString]) -> Result<Self, Box<dyn std::error::Error>> {
        let repeat = arguments
            .last()
            .is_some_and(|value| value == OsStr::new("--repeat"));
        let arguments = if repeat {
            &arguments[..arguments.len() - 1]
        } else {
            arguments
        };
        if !(2..=3).contains(&arguments.len()) {
            return Err("expected network target [latestOrCanonicalBlockHash] [--repeat]".into());
        }
        let text = arguments
            .iter()
            .map(|value| {
                value
                    .to_str()
                    .ok_or("non-UTF8 source qualification argument")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let network = match text[0] {
            "ethereumSepolia" => EvmNetwork::EthereumSepolia,
            "baseSepolia" => EvmNetwork::BaseSepolia,
            "ethereumMainnet" => EvmNetwork::EthereumMainnet,
            "baseMainnet" => EvmNetwork::BaseMainnet,
            _ => return Err("unsupported source qualification network".into()),
        };
        Ok(Self {
            network,
            target: PublicEvmAddress::new(text[1])?,
            block: ContractBlock::new(text.get(2).copied().unwrap_or("latest"))?,
            repeat,
        })
    }
}

pub(crate) fn run(arguments: &[OsString]) -> Result<(), Box<dyn std::error::Error>> {
    let request = Request::parse(arguments)?;
    // The configured URL stays in the native owner. It never enters arguments,
    // stdout, model context or diagnostics. No wallet/profile is initialised.
    eprintln!("Source qualification: resolving native RPC selection");
    let endpoint =
        crate::evm_rpc_settings::resolved(request.network).map_err(|error| error.message)?;
    eprintln!(
        "Source qualification RPC origin: {}",
        endpoint.public_origin()
    );
    eprintln!("Source qualification: starting proof and source inspection");
    let cache = std::env::temp_dir().join(format!(
        "minutus-source-diagnostic-{}",
        uuid::Uuid::now_v7()
    ));
    let count = if request.repeat { 3 } else { 1 };
    let mut failed = false;
    for attempt in 1..=count {
        eprintln!("Source qualification attempt: {attempt}/{count}");
        match inspect(&request, &endpoint, &cache, attempt) {
            Ok(facts) => println!("{}", output(&endpoint, &facts)?),
            Err(error) => {
                eprintln!("Source qualification attempt {attempt} failed: {error}");
                failed = true;
            }
        }
    }
    if request.repeat {
        // Run the existing ordinary-read transport only as a labelled control
        // after all source trials. It cannot turn their failures into success.
        let started = Instant::now();
        match observe_contract_code_with_admission(
            request.network,
            &endpoint,
            request.target.as_str(),
            &request.block,
            &|| true,
        ) {
            Ok(observation) => println!(
                "{}",
                json!({
                "mode":"unsignedRuntimeControl","rpcOrigin":endpoint.public_origin(),
                "elapsedMs":started.elapsed().as_millis(),
                "noProfileVaultSigningOrSend":true,"sourceQualification":{"status":"notRequested"},
                "effectsQualified":false,"writeAuthorityGranted":false,
                "blockHash":observation.block_hash,"blockNumber":observation.block_number,
                "runtimeCodeBytes":observation.runtime_code_bytes,
                "runtimeCodeHash":observation.runtime_code_hash})
            ),
            Err(_) => eprintln!(
                "Ordinary runtime control failed; elapsedMs={}",
                started.elapsed().as_millis()
            ),
        }
    }
    if failed {
        return Err("one or more independent source qualification attempts failed".into());
    }
    Ok(())
}

fn inspect(
    request: &Request,
    endpoint: &EvmRpcUrl,
    cache: &Path,
    attempt: u8,
) -> Result<Value, GenericError> {
    // Explicit repeat mode is an independent read-only experiment: endpoint
    // resolution/Keychain happens once, while every trial runs normal fresh
    // inspection with a new provider. Production tools gain no retry route.
    let mut sources = GenericSourceProvider::packaged(cache)?;
    let observation = inspect_generic_source(
        request.network,
        endpoint,
        request.target.as_str(),
        &request.block,
        &mut sources,
        &AtomicBool::new(false),
    )?;
    Ok(
        json!({"diagnosticAttempt":attempt,"network":observation.network().descriptor().caip2_id(),
        "target":observation.target(),"blockHash":observation.block_hash(),
        "blockNumber":observation.block_number().to_string(),"sourceCorrespondence":observation.metadata()}),
    )
}

fn output(endpoint: &EvmRpcUrl, facts: &Value) -> Result<String, Box<dyn std::error::Error>> {
    let value = json!({"mode":"unsignedSourceQualification","rpcOrigin":endpoint.public_origin(),
        "usesNativeRpcSettings":true,"noProfileVaultSigningOrSend":true,
        "sourceQualification":{"status":"reproduced"},"effectsQualified":false,
        "writeAuthorityGranted":false,"observation":facts});
    let text = serde_json::to_string(&value)?;
    if text.len() > 64 * 1024 {
        return Err("complete source diagnostic exceeds 64 KiB".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeat_is_explicit_bounded_and_retains_latest_per_attempt()
    -> Result<(), Box<dyn std::error::Error>> {
        let args = [
            "ethereumSepolia",
            "0x1111111111111111111111111111111111111111",
            "--repeat",
        ]
        .map(OsString::from);
        let request = Request::parse(&args)?;
        assert!(request.repeat);
        assert!(matches!(request.block, ContractBlock::Latest));
        assert!(!Request::parse(&args[..2])?.repeat);
        Ok(())
    }
    #[test]
    fn request_rejects_bad_inputs_before_native_endpoint_resolution() {
        for args in [
            vec![],
            vec!["sepolia", "0x1111111111111111111111111111111111111111"],
            vec!["ethereumSepolia", "not-an-address"],
            vec![
                "ethereumSepolia",
                "0x1111111111111111111111111111111111111111",
                "123",
            ],
        ] {
            assert!(
                Request::parse(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).is_err()
            );
        }
    }
    #[test]
    fn output_contains_only_public_origin_and_bounded_unsigned_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let endpoint = EvmRpcUrl::new(
            "https://user:secret@provider.example/private-key?token=credential".into(),
        )?;
        let text = output(&endpoint, &json!({"blockHash":"public-block"}))?;
        let value: Value = serde_json::from_str(&text)?;
        assert_eq!(value["rpcOrigin"], "https://provider.example");
        assert_eq!(value["writeAuthorityGranted"], false);
        assert_eq!(value["effectsQualified"], false);
        for secret in ["user:secret", "private-key", "credential"] {
            assert!(!text.contains(secret));
        }
        assert!(output(&endpoint, &json!({"abi":"x".repeat(64*1024)})).is_err());
        Ok(())
    }
}
