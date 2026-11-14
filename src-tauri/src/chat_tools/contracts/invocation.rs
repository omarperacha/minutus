//! Local hypothetical invocation data, kept separate from outbound `eth_call`.
use chain_evm::{
    EvmNetwork, PublicEvmAddress,
    contract::{DeclaredContractCall, UnsignedContractIntent},
};
use ipc_types::IpcError;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct InvocationIntent {
    function: String,
    arguments: Vec<Value>,
    caller: String,
    #[serde(default, deserialize_with = "present")]
    origin: Option<String>,
    #[serde(default = "zero")]
    value_wei: String,
}

fn zero() -> String {
    "0".into()
}

/// Optional fields may be absent, but an explicitly supplied null is invalid.
pub(super) fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

pub(super) struct LocalInvocation {
    pub(super) intent: UnsignedContractIntent,
    pub(super) origin: String,
}

impl InvocationIntent {
    pub(super) fn validate(
        self,
        network: EvmNetwork,
        target: &str,
    ) -> Result<LocalInvocation, IpcError> {
        let call = DeclaredContractCall::new(target, &self.function, &self.arguments, &[])
            .map_err(super::intent::contract_error)?;
        let intent =
            UnsignedContractIntent::new_declared(network, &self.caller, &self.value_wei, call)
                .map_err(super::intent::contract_error)?;
        let origin = PublicEvmAddress::new(self.origin.as_deref().unwrap_or(intent.account()))
            .map_err(|_| IpcError::invalid_request("Expected an exact public origin address"))?;
        Ok(LocalInvocation {
            intent,
            origin: origin.as_str().into(),
        })
    }
}

#[cfg(test)]
mod tests;
