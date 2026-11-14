use chain_evm::{
    EvmNetwork, PublicEvmAddress,
    contract::{ContractBlock, ContractError, DeclaredContractCall},
};
use ipc_types::{IpcError, IpcErrorCode};
use model_runtime::ParsedToolCall;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetadataIntent {
    network: ipc_types::EvmNetwork,
    target: String,
    block: String,
    #[serde(default, rename = "includeSource")]
    include_source: bool,
    #[serde(default, deserialize_with = "super::invocation::present")]
    invocation: Option<super::invocation::InvocationIntent>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadIntent {
    network: ipc_types::EvmNetwork,
    target: String,
    block: String,
    caller: Option<String>,
    function: String,
    arguments: Vec<serde_json::Value>,
    outputs: Vec<String>,
}

pub(super) struct ObservationRequest {
    pub(super) include_source: bool,
    pub(super) network: EvmNetwork,
    pub(super) target: String,
    pub(super) block: ContractBlock,
    pub(super) call: Option<DeclaredContractCall>,
    pub(super) caller: Option<String>,
    pub(super) invocation: Option<super::invocation::LocalInvocation>,
}
fn parse<T: serde::de::DeserializeOwned>(call: &ParsedToolCall) -> Result<T, IpcError> {
    serde_json::from_value(json!(call.arguments)).map_err(|_| IpcError::invalid_request("Use the exact advertised contract observation fields; unknown fields and unsupported ABI types are forbidden"))
}
fn address(value: &str) -> Result<String, IpcError> {
    PublicEvmAddress::new(value)
        .map(|address| address.as_str().into())
        .map_err(|_| IpcError::invalid_request("Expected an exact hexadecimal public address"))
}

pub(super) fn request(call: &ParsedToolCall) -> Result<ObservationRequest, IpcError> {
    match call.name.as_str() {
        "contract.inspect" => {
            let intent: MetadataIntent = parse(call)?;
            if intent.invocation.is_some() && !intent.include_source {
                return Err(IpcError::invalid_request(
                    "invocation requires includeSource=true; it is local discovery, not eth_call",
                ));
            }
            let network = crate::evm_rpc_settings::chain_network(intent.network);
            let target = address(&intent.target)?;
            let invocation = intent
                .invocation
                .map(|invocation| invocation.validate(network, &target))
                .transpose()?;
            Ok(ObservationRequest {
                include_source: intent.include_source,
                network,
                target,
                block: ContractBlock::new(&intent.block).map_err(contract_error)?,
                call: None,
                caller: None,
                invocation,
            })
        }
        "contract.read" => {
            let intent: ReadIntent = parse(call)?;
            let call = DeclaredContractCall::new(
                &intent.target,
                &intent.function,
                &intent.arguments,
                &intent.outputs,
            )
            .map_err(contract_error)?;
            Ok(ObservationRequest {
                include_source: false,
                network: crate::evm_rpc_settings::chain_network(intent.network),
                target: call.target().into(),
                block: ContractBlock::new(&intent.block).map_err(contract_error)?,
                call: Some(call),
                caller: intent.caller.as_deref().map(address).transpose()?,
                invocation: None,
            })
        }
        _ => Err(IpcError::invalid_request("Unavailable contract operation")),
    }
}
pub(in crate::chat_tools) fn validate_call(call: &ParsedToolCall) -> Result<(), IpcError> {
    request(call).map(|_| ())
}

pub(super) fn contract_error(error: ContractError) -> IpcError {
    let (code, retryable) = match error {
        ContractError::Cancelled => (IpcErrorCode::Cancelled, false),
        ContractError::ProviderUnavailable | ContractError::NonCanonicalBlock => {
            (IpcErrorCode::Unsupported, true)
        }
        ContractError::WrongChain | ContractError::NoCode => (IpcErrorCode::Unsupported, false),
        _ => (IpcErrorCode::InvalidRequest, false),
    };
    IpcError {
        code,
        message: error.to_string(),
        retryable,
    }
}
