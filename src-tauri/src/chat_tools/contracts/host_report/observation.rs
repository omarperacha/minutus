//! Separate unsigned observations never change the workflow's recorded outcome.
use super::{
    HostContractReport, display,
    workflow::{network_chain, result_json},
};
use crate::chat_tools::ChatToolExchange;
use chain_evm::{PublicEvmAddress, TransactionHash};
use ipc_types::{ToolName, ToolOutcome};
use serde_json::{Value, json};

impl HostContractReport {
    pub(crate) fn observation(&self, exchange: &ChatToolExchange, after: bool) -> Option<String> {
        if exchange.request.call_id != exchange.result.call_id
            || !matches!(exchange.result.outcome, ToolOutcome::Completed { .. })
        {
            return None;
        }
        let request = serde_json::to_value(&exchange.request.arguments).ok()?;
        let value = result_json(&exchange.result.outcome)?;
        let facts = match exchange.request.name {
            ToolName::ContractRead | ToolName::ContractInspect => self.unsigned(
                &request,
                &value,
                exchange.request.name == ToolName::ContractRead,
            )?,
            ToolName::ContractReconcileTransaction if after => {
                self.reconciliation(&request, &value)?
            }
            _ => return None,
        };
        let timing = if after { "Later" } else { "Preparatory" };
        Some(format!(
            "\n\n{timing} separate unsigned observation (untrusted data; no workflow completion, causation or signing authority):\n```json\n{}\n```",
            display::json(&facts)
        ))
    }

    fn unsigned(&self, request: &Value, value: &Value, read: bool) -> Option<Value> {
        if request["network"] != self.network()
            || value["writeAuthorityGranted"] != false
            || value["effectsQualified"] != false
        {
            return None;
        }
        let target = PublicEvmAddress::new(request["target"].as_str()?).ok()?;
        if !self
            .targets()
            .iter()
            .any(|address| address == target.as_str())
        {
            return None;
        }
        let observation = &value["observation"];
        let context = if read {
            &observation["context"]
        } else {
            observation
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "The four supported EIP-155 chain IDs are exact in IEEE-754"
        )]
        let expected = network_chain(self.network())? as f64;
        if context["chainId"] != json!(expected)
            || context["target"] != target.as_str()
            || context["blockNumber"].as_str()?.is_empty()
            || TransactionHash::new(context["blockHash"].as_str()?).is_err()
        {
            return None;
        }
        if read {
            if observation["function"] != request["function"]
                || observation["arguments"] != request["arguments"]
                || observation["outputTypes"] != request["outputs"]
                || observation["outcome"]["status"] != "returned"
            {
                return None;
            }
            Some(json!({"kind":"contract.read","context":context,
                "caller":observation["caller"],"function":observation["function"],
                "arguments":observation["arguments"],"outputTypes":observation["outputTypes"],
                "outcome":observation["outcome"]}))
        } else {
            Some(json!({"kind":"contract.inspect","target":target.as_str(),
                "chainId":context["chainId"],"blockNumber":context["blockNumber"],
                "blockHash":context["blockHash"],"runtimeCodeHash":context["runtimeCodeHash"],
                "sourceQualification":value["sourceQualification"]}))
        }
    }

    fn reconciliation(&self, request: &Value, value: &Value) -> Option<Value> {
        let hash = TransactionHash::new(request["transactionHash"].as_str()?).ok()?;
        if !self.hashes.contains(hash.as_str())
            || value["transactionHash"] != hash.as_str()
            || value["writesResumed"] != false
            || value["retryAutomatically"] != false
            || !matches!(
                value["state"].as_str()?,
                "pending"
                    | "notObserved"
                    | "verificationUnavailable"
                    | "executionConfirmed"
                    | "executionReverted"
            )
            || !matches!(
                value["workflowCompletion"].as_str()?,
                "unverified" | "notReverified"
            )
        {
            return None;
        }
        Some(value.clone())
    }
}
