//! Request-local terminal projection, after ordinary plan/evidence admission.
use super::{
    AppCore, ChatTerminal, Conversation, ExecutionProfileDecision, FinishReason, IpcError,
    ParsedToolCall, PlanPolicy,
};
use crate::chat_tools::{ChatToolExchange, HostContractReport, contract_host_report};
use model_runtime::GenerationBinding;

const MAX_REPORT_BYTES: usize = 8 * 1024;
const MAX_RETAINED_EXCHANGES: usize = 32;

#[derive(Default)]
pub(super) struct ContractReportState {
    binding: Option<GenerationBinding>,
    revision: Option<usize>,
    exchanges: Vec<ChatToolExchange>,
    excluded: bool,
}

impl ContractReportState {
    pub(super) fn bind(&mut self, binding: &GenerationBinding) {
        self.binding = Some(binding.clone());
    }

    pub(super) fn record(
        &mut self,
        revision: usize,
        calls: &[ParsedToolCall],
        results: &[Result<ChatToolExchange, IpcError>],
    ) {
        if calls.len() != results.len() {
            self.excluded = true;
            return;
        }
        for (call, result) in calls.iter().zip(results) {
            if matches!(
                call.name.as_str(),
                "orchestration.submit_plan"
                    | "orchestration.submit_forecast"
                    | "orchestration.complete_stage"
                    | "orchestration.search_tools"
            ) {
                continue;
            }
            if self.revision == Some(0)
                && revision == 1
                && self
                    .exchanges
                    .iter()
                    .all(|exchange| exchange.request.name == ipc_types::ToolName::ContractInspect)
            {
                // Metadata discovery may precede the first plan. This is not a
                // revision of admitted work, and all observations stay scoped.
                self.revision = Some(1);
            } else if self.revision.is_some_and(|old| old != revision) {
                self.excluded = true;
            }
            self.revision.get_or_insert(revision);
            let Ok(exchange) = result else {
                self.excluded = true;
                continue;
            };
            if !matches!(
                call.name.as_str(),
                "contract.request_workflow"
                    | "contract.read"
                    | "contract.inspect"
                    | "contract.reconcile_transaction"
            ) || call.recipient != call.name
                || call.call_id != exchange.request.call_id.0
                || serde_json::to_value(&exchange.request.name).ok().as_ref()
                    != Some(&serde_json::Value::String(call.name.clone()))
                || self.exchanges.len() >= MAX_RETAINED_EXCHANGES
            {
                self.excluded = true;
                continue;
            }
            self.exchanges.push(exchange.clone());
        }
    }

    fn render(&self, revision: usize) -> Option<String> {
        if self.excluded || self.revision != Some(revision) {
            return None;
        }
        let reports: Vec<(usize, HostContractReport)> = self
            .exchanges
            .iter()
            .enumerate()
            .filter_map(|(index, exchange)| {
                contract_host_report(exchange).map(|report| (index, report))
            })
            .collect();
        let [(position, report)] = reports.as_slice() else {
            return None;
        };
        let mut rendered = report.render().to_owned();
        for (index, exchange) in self.exchanges.iter().enumerate() {
            if index != *position {
                rendered.push_str(&report.observation(exchange, index > *position)?);
            }
            if rendered.len() > MAX_REPORT_BYTES {
                return None;
            }
        }
        Some(rendered)
    }

    pub(super) fn terminal(
        &self,
        core: &AppCore,
        conversation: &Conversation,
        policy: &PlanPolicy,
        profile: ExecutionProfileDecision,
    ) -> Option<ChatTerminal> {
        if !policy.contract_report_ready() {
            return None;
        }
        let rendered = self.render(policy.revision())?;
        if !core
            .admit_host_final_report(conversation, self.binding.as_ref()?, &rendered, profile)
            .ok()?
        {
            return None;
        }
        Some(ChatTerminal::Final {
            reasoning: None,
            text: rendered,
            reason: FinishReason::Stop,
            mutation_applied: false,
        })
    }
}

#[cfg(test)]
mod tests;
