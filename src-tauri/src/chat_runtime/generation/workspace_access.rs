//! Stop declared workspace effects when no project folder grants write access.
use super::{
    ChatTerminal, GenerationFailureCategory, GenerationToolState, IpcError, terminal_error,
};

pub(super) fn require_planned_write_access(
    state: &GenerationToolState,
    writable: bool,
) -> Result<(), ChatTerminal> {
    if state.plan_policy.expects_file_mutation() && !writable {
        return Err(terminal_error(
            IpcError::permission_denied(
                "Minutus cannot create or edit project files because no project folder has Read and write access. Open Project settings > Folder access, add a folder or enable Read and write for the intended folder, then retry. No file was written.",
            ),
            GenerationFailureCategory::ToolValidation,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use model_runtime::{ParsedToolCall, PromptToolDefinition};
    use std::collections::BTreeMap;

    fn state(outcome: &str) -> Result<GenerationToolState, IpcError> {
        let definition = PromptToolDefinition {
            name: "computation.run".into(),
            description: "Pure computation".into(),
            parameters: serde_json::json!({"type":"object"}),
        };
        let arguments = serde_json::json!({
            "assumptions":[], "constraints":[], "contractVersion":"semantic_execution_v6",
            "mode":"initial", "outcome":outcome, "target":"Requested result",
            "successCriteria":["Requested result delivered"],
            "steps":[{"actionClass":if outcome == "answer" { "compute" } else { "workspace_write" }, "dependsOn":[], "doneWhen":"Result obtained",
                "id":"work", "objective":"Obtain requested result", "suggestedTools":["computation.run"]}]
        });
        let call = ParsedToolCall {
            call_id: "plan".into(),
            recipient: "orchestration.submit_plan".into(),
            name: "orchestration.submit_plan".into(),
            arguments: serde_json::from_value::<BTreeMap<String, serde_json::Value>>(arguments)
                .map_err(|_| IpcError::internal())?,
        };
        let mut state = GenerationToolState::default();
        state.plan_policy.accept(&call, &[definition], 16)?;
        Ok(state)
    }

    #[test]
    fn declared_file_outcome_stops_before_another_round_or_sibling_execution()
    -> Result<(), IpcError> {
        for outcome in ["create_file", "modify_file"] {
            let state = state(outcome)?;
            let Err(ChatTerminal::Error { error, .. }) =
                require_planned_write_access(&state, false)
            else {
                return Err(IpcError::internal());
            };
            assert_eq!(error.code, ipc_types::IpcErrorCode::PermissionDenied);
            assert!(error.message.contains("Project settings > Folder access"));
            assert!(!error.retryable);
            assert!(require_planned_write_access(&state, true).is_ok());
        }
        Ok(())
    }

    #[test]
    fn read_only_work_and_unplanned_dialogue_are_not_blocked() -> Result<(), IpcError> {
        assert!(require_planned_write_access(&GenerationToolState::default(), false).is_ok());
        assert!(require_planned_write_access(&state("answer")?, false).is_ok());
        Ok(())
    }
}
