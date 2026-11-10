//! Versioned model declarations. No provider, nonce, gas, compiler or proof input.
use app_core::{
    ContractWorkflowDeclaration, StoredDeclaredWorkflowStep, StoredWorkflowCondition,
    StoredWorkflowContract, StoredWorkflowDeclaration, StoredWorkflowRead,
};
use ipc_types::IpcError;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DeclaredWorkflowRequest {
    pub(crate) contract_version: u8,
    pub(crate) network: ipc_types::EvmNetwork,
    pub(crate) contracts: Vec<StoredWorkflowContract>,
    pub(crate) steps: Vec<DeclaredWorkflowRequestStep>,
    pub(crate) goals: Vec<StoredWorkflowCondition>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DeclaredWorkflowRequestStep {
    id: String,
    contract: String,
    function: String,
    arguments: Vec<Value>,
    value_wei: String,
    depends_on: Vec<String>,
    prerequisites: Vec<StoredWorkflowCondition>,
    completion: Vec<StoredWorkflowCondition>,
}
impl DeclaredWorkflowRequest {
    pub(crate) fn validate(&self) -> Result<(), IpcError> {
        self.declaration("0x1111111111111111111111111111111111111111")
            .map(|_| ())
    }
    pub(super) fn declaration(
        &self,
        account: &str,
    ) -> Result<ContractWorkflowDeclaration, IpcError> {
        if self.contract_version != 2
            || self.steps.is_empty()
            || self.steps.len() > 16
            || self.contracts.is_empty()
            || self.contracts.len() > 16
        {
            return Err(IpcError::invalid_request(
                "Expected generic workflow version2 with1 to16 contracts and steps",
            ));
        }
        let mut prior = BTreeMap::new();
        let mut steps = Vec::with_capacity(self.steps.len());
        for (index, step) in self.steps.iter().enumerate() {
            let depends_on = step
                .depends_on
                .iter()
                .map(|id| {
                    prior.get(id.as_str()).copied().ok_or_else(|| {
                        IpcError::invalid_request(
                            "Workflow dependencies must name distinct earlier steps",
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if prior
                .insert(
                    step.id.as_str(),
                    u8::try_from(index)
                        .map_err(|_| IpcError::invalid_request("Too many workflow steps"))?,
                )
                .is_some()
            {
                return Err(IpcError::invalid_request(
                    "Workflow step identifiers must be unique",
                ));
            }
            steps.push(StoredDeclaredWorkflowStep {
                id: step.id.clone(),
                contract: step.contract.clone(),
                function: step.function.clone(),
                arguments: step.arguments.clone(),
                value_wei: step.value_wei.clone(),
                depends_on,
                prerequisites: step.prerequisites.clone(),
                completion: step.completion.clone(),
            });
        }
        let declaration = ContractWorkflowDeclaration::new(StoredWorkflowDeclaration {
            version: 2,
            network: crate::evm_rpc_settings::chain_network(self.network)
                .descriptor()
                .caip2_id()
                .into(),
            account: account.into(),
            contracts: self.contracts.clone(),
            steps,
            goals: self.goals.clone(),
        })
        .map_err(|error| crate::error_mapping::app_core_error(error.into()))?;
        if declaration
            .intent()
            .steps
            .iter()
            .flat_map(|step| step.prerequisites.iter().chain(&step.completion))
            .chain(&declaration.intent().goals)
            .any(|condition| !matches!(condition.read, StoredWorkflowRead::SourceField { .. }))
        {
            return Err(IpcError::invalid_request(
                "Workflow conditions currently require an exact source field path",
            ));
        }
        Ok(declaration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn request() -> Value {
        json!({"contractVersion":2,"network":"ethereumSepolia","contracts":[{"id":"app","address":"0x2222222222222222222222222222222222222222"}],"steps":[{"id":"configure","contract":"app","function":"configure((uint256,string)[])","arguments":[[["7","a"]]],"valueWei":"0","dependsOn":[],"prerequisites":[],"completion":[condition()]}],"goals":[condition()]})
    }
    fn condition() -> Value {
        json!({"contract":"app","read":{"kind":"sourceField","path":["remaining"]},"comparison":"equal","expected":"7"})
    }
    #[test]
    fn arbitrary_abi_declarations_admit_without_a_function_catalogue()
    -> Result<(), Box<dyn std::error::Error>> {
        let parsed: DeclaredWorkflowRequest = serde_json::from_value(request())?;
        parsed.validate().map_err(|e| e.message)?;
        let declaration = parsed
            .declaration("0xabcdefabcdefabcdefabcdefabcdefabcdefabcd")
            .map_err(|e| e.message)?;
        assert_eq!(
            declaration.intent().steps[0].function,
            "configure((uint256,string)[])"
        );
        assert_eq!(
            declaration.intent().steps[0].arguments,
            json!([[["7", "a"]]])
                .as_array()
                .ok_or("arguments")?
                .as_slice()
        );
        Ok(())
    }
    #[test]
    fn declarations_reject_privileged_fields_and_unknown_dependencies()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut value = request();
        value["gasLimit"] = json!(1);
        assert!(serde_json::from_value::<DeclaredWorkflowRequest>(value).is_err());
        let mut value = request();
        value["steps"][0]["dependsOn"] = json!(["configure"]);
        assert!(
            serde_json::from_value::<DeclaredWorkflowRequest>(value)?
                .validate()
                .is_err()
        );
        let mut value = request();
        value["steps"][0]["completion"] = json!([]);
        value["goals"] = json!([]);
        let declared = serde_json::from_value::<DeclaredWorkflowRequest>(value)?;
        declared.validate().map_err(|error| error.message)?;
        assert!(declared.goals.is_empty());
        Ok(())
    }
}
