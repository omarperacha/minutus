//! Fixed conclusions from host states, never from model prose or getter values.
use super::{HostContractReport, workflow::result_json};
use crate::chat_tools::ChatToolExchange;
use ipc_types::ToolName;

impl HostContractReport {
    pub(crate) fn status_summary(&self, exchanges: &[ChatToolExchange]) -> Option<String> {
        let mut latest = None;
        for exchange in exchanges {
            if exchange.request.name == ToolName::ContractReconcileTransaction {
                let request = serde_json::to_value(&exchange.request.arguments).ok()?;
                let value = result_json(&exchange.result.outcome)?;
                latest = Some(self.reconciliation(&request, &value)?);
            }
        }
        let execution = if let Some(value) = &latest {
            let hash = value["transactionHash"].as_str()?;
            match value["state"].as_str()? {
                "executionConfirmed" => format!(
                    "Execution confirmed for {hash} by canonical transaction/receipt inclusion at block {}. This is not finality.",
                    value["receiptBlockNumber"].as_str()?
                ),
                "executionReverted" => format!(
                    "Execution reverted for {hash}, proved at receipt block {}. This is not workflow completion.",
                    value["receiptBlockNumber"].as_str()?
                ),
                "pending" => format!(
                    "Latest reconciliation reports {hash} pending; confirmed execution is not established by that observation."
                ),
                "notObserved" => format!(
                    "Latest reconciliation did not observe {hash}; execution or replacement is not established."
                ),
                "verificationUnavailable" => format!(
                    "Latest reconciliation could not verify {hash}; prior inclusion facts remain historical."
                ),
                _ => return None,
            }
        } else if self.inclusion_verified {
            "Canonical inclusion is recorded for at least one workflow transaction; each transaction's facts remain separate below.".into()
        } else {
            "Canonical transaction inclusion is not established by this workflow report; submission and receipt reports remain separate facts.".into()
        };
        let formal = if latest.is_some() {
            "Formal workflow completion: unverified after receipt-only reconciliation; it was not reverified. Current declared goals were not rechecked."
        } else if self.workflow_verified {
            "Formal workflow completion: verified at the workflow's listed proof observations; later current state is not established."
        } else {
            "Formal workflow completion: unverified. Confirmed execution or a desired getter value cannot establish it."
        };
        Some(format!(
            "Authoritative outcome:\n{execution}\n{formal}\nSeparate unsigned reads retain their own values and blocks; they do not establish workflow completion or causation."
        ))
    }
}
