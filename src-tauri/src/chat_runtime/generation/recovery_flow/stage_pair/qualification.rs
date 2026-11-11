//! Scripted packaged schema recovery; no execution, wallet or network composition.
use super::{GenerationToolState, IpcError, IpcErrorCode, ParsedToolCall, prepare};
use crate::chat_tools::{
    ChatToolExchange, chat_tool_definitions, exact_argument_repair_error,
    validated_chat_tool_request,
};
use ipc_types::{TaskActivityStatus, ToolOutcome, ToolResult};
use model_runtime::PromptToolDefinition;
use serde_json::{Value, json};
use std::ffi::OsString;

const COMPARISON_PATH: &str = "$.steps[0].prerequisites[0].comparison";
struct DiagnosticCase {
    plan: ParsedToolCall,
    calls: Vec<ParsedToolCall>,
    prior: Value,
}

pub(crate) fn run(arguments: &[OsString]) -> Result<String, IpcError> {
    require(
        arguments.is_empty(),
        "This fixed diagnostic accepts no arguments",
    )?;
    let catalogue = chat_tool_definitions(&[])?;
    require(
        catalogue
            .iter()
            .any(|tool| tool.name == "contract.request_workflow"),
        "The genuine packaged catalogue does not advertise contract workflows",
    )?;
    let DiagnosticCase { plan, calls, prior } = fixture()?;
    let mut state = initial_state(&plan, &catalogue, &prior)?;
    let before = state.plan_policy.progress_projection(false);
    let revision = state.plan_policy.revision();
    let evidence = state.operation_ledger.useful_result_sequence();
    require(
        evidence > 0,
        "The captured historical read was not retained as useful evidence",
    )?;
    let budget = state.total_calls;
    let original = super::super::tool_call_fingerprint(&calls);
    let invalid = invalid_calls(&calls);
    require(
        matches!(invalid.as_slice(), [(1, _)]),
        "Expected exactly one malformed task",
    )?;
    let error = exact_argument_repair_error(&calls[1], &catalogue, &invalid[0].1);
    require(
        error.code == IpcErrorCode::InvalidRequest
            && error.message.contains(COMPARISON_PATH)
            && error.message.contains("[\"equal\",\"atLeast\",\"atMost\"]"),
        "The production validator did not identify the exact comparison enum",
    )?;
    let pair = prepare(&mut state, &calls, &invalid, &catalogue, &error)
        .map_err(|_| IpcError::invalid_request("The scripted correction became terminal"))?
        .ok_or_else(|| IpcError::invalid_request("The scripted pair was not repairable"))?;
    verify_pair(pair.exchanges(), &calls)?;
    require(
        super::super::tool_call_fingerprint(&calls) == original
            && state.plan_policy.progress_projection(false) == before
            && state.plan_policy.revision() == revision
            && state.operation_ledger.useful_result_sequence() == evidence
            && state.plan_policy.current_stage_id() == Some("read-current")
            && state.argument_corrections == 1
            && state.total_calls > budget
            && state.recovery.pending().is_some(),
        "Rejected calls changed retained state or escaped correction accounting",
    )?;
    verify_authored_correction(&mut state, &calls)?;
    output()
}

fn fixture() -> Result<DiagnosticCase, IpcError> {
    let data: Value =
        serde_json::from_str(include_str!("tests/fixtures/sepolia-stage-workflow.json"))
            .map_err(|_| IpcError::internal())?;
    let plan = ParsedToolCall {
        call_id: "scripted-plan".into(),
        name: "orchestration.submit_plan".into(),
        recipient: "orchestration.submit_plan".into(),
        arguments: serde_json::from_value(data["plan"].clone())
            .map_err(|_| IpcError::internal())?,
    };
    let calls = data["calls"]
        .as_array()
        .ok_or_else(IpcError::internal)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            Ok(ParsedToolCall {
                call_id: format!("scripted-pair-{index}"),
                name: value["name"]
                    .as_str()
                    .ok_or_else(IpcError::internal)?
                    .into(),
                recipient: value["recipient"]
                    .as_str()
                    .ok_or_else(IpcError::internal)?
                    .into(),
                arguments: serde_json::from_value(value["arguments"].clone())
                    .map_err(|_| IpcError::internal())?,
            })
        })
        .collect::<Result<Vec<_>, IpcError>>()?;
    require(calls.len() == 2, "Expected the fixed two-call fixture")?;
    Ok(DiagnosticCase {
        plan,
        calls,
        prior: data["priorRead"].clone(),
    })
}

fn initial_state(
    plan: &ParsedToolCall,
    catalogue: &[PromptToolDefinition],
    prior: &Value,
) -> Result<GenerationToolState, IpcError> {
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(plan, catalogue, 20)?;
    // This fixed historical observation is replayed into bookkeeping only.
    // No current read, proof, source certificate or execution occurs here.
    let captured: ipc_types::ToolRequest =
        serde_json::from_value(prior["request"].clone()).map_err(|_| IpcError::internal())?;
    let read = ParsedToolCall {
        call_id: captured.call_id.0,
        name: "contract.read".into(),
        recipient: "contract.read".into(),
        arguments: serde_json::from_value(prior["request"]["arguments"].clone())
            .map_err(|_| IpcError::internal())?,
    };
    let request = validated_chat_tool_request(&read, &[])?;
    let result: ToolResult =
        serde_json::from_value(prior["result"].clone()).map_err(|_| IpcError::internal())?;
    require(
        result.call_id == request.call_id
            && matches!(&result.outcome,ToolOutcome::Completed {content} if !content.is_empty()),
        "The fixed prior receipt was not paired completed historical evidence",
    )?;
    let exchange = ChatToolExchange {
        result,
        activity: ipc_types::TaskActivity {
            status: TaskActivityStatus::Completed,
            ..crate::chat_runtime::tool_activity(&read.call_id, &read.name)
        },
        request,
        workspace_grant_id: None,
        undo_changes: vec![],
        inspected_asset_id: None,
        computation_reuse_digest: None,
    };
    state.record_results(&[read], &[Ok(exchange)]);
    state.observe_recovery_checkpoint();
    Ok(state)
}

fn invalid_calls(calls: &[ParsedToolCall]) -> Vec<(usize, IpcError)> {
    calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            validated_chat_tool_request(call, &[])
                .err()
                .map(|error| (index, error))
        })
        .collect()
}

fn verify_pair(
    exchanges: &[ChatToolExchange; 2],
    calls: &[ParsedToolCall],
) -> Result<(), IpcError> {
    for (exchange, call) in exchanges.iter().zip(calls) {
        require(
            matches!(exchange.result.outcome, ToolOutcome::Failed { .. })
                && exchange.activity.status == TaskActivityStatus::Partial
                && exchange.request.call_id.0 == call.call_id
                && exchange.workspace_grant_id.is_none()
                && exchange.undo_changes.is_empty(),
            "A rejected pair created an executed or unpaired receipt",
        )?;
    }
    let ToolOutcome::Failed { error: control } = &exchanges[0].result.outcome else {
        return Err(IpcError::internal());
    };
    let ToolOutcome::Failed { error: task } = &exchanges[1].result.outcome else {
        return Err(IpcError::internal());
    };
    require(
        control.message.contains("Neither call was executed")
            && task.message.contains(COMPARISON_PATH)
            && exchanges[1].request.arguments.get("steps")
                == Some(
                    &serde_json::from_value::<ipc_types::JsonValue>(
                        calls[1].arguments["steps"].clone(),
                    )
                    .map_err(|_| IpcError::internal())?,
                ),
        "Rejected receipts lost the original arguments or exact correction",
    )
}

fn verify_authored_correction(
    state: &mut GenerationToolState,
    calls: &[ParsedToolCall],
) -> Result<(), IpcError> {
    // This fixed alternative is a NEW preauthored fixture decision. It is not
    // host normalisation of an emitted call and is not sent to any executor.
    let mut corrected = calls.to_vec();
    let steps = corrected[1]
        .arguments
        .get_mut("steps")
        .ok_or_else(IpcError::internal)?;
    steps[0]["prerequisites"][0]["comparison"] = json!("equal");
    steps[0]["prerequisites"][0]["expected"] = json!("1");
    for call in &corrected {
        validated_chat_tool_request(call, &[])?;
    }
    require(
        state.plan_policy.validate_calls(&corrected[1..]).is_err(),
        "The current read stage admitted a write",
    )?;
    state.plan_policy.complete_advisory_stage(&corrected[0])?;
    state.plan_policy.validate_calls(&corrected[1..])?;
    require(
        state.plan_policy.current_stage_id() == Some("store-write"),
        "Explicit stage completion did not establish ordinary write-stage readiness",
    )
}

fn require(condition: bool, message: &str) -> Result<(), IpcError> {
    if condition {
        Ok(())
    } else {
        Err(IpcError::invalid_request(message))
    }
}
fn output() -> Result<String, IpcError> {
    let text = json!({"mode":"scriptedStageArgumentRecovery","packagedCatalogueUsed":true,
        "comparisonPath":COMPARISON_PATH,"allowedComparisons":["equal","atLeast","atMost"],
        "pairedPartialFailures":2,"bothCallsUnexecuted":true,"originalArgumentsPreserved":true,
        "planRevisionAndEvidencePreserved":true,"requestAndCorrectionBudgetsCharged":true,
        "priorReadReceipt":"fixed historical observation playback only; no current RPC observation",
        "correctedCall":"separate preauthored fixture; not host argument normalisation",
        "correctedSchemaValid":true,"explicitCompletionChangesPlanReadinessOnly":true,
        "profileWalletKeychainRpcNativePromptSigningOrSendAccessed":false,
        "writeAuthorityGranted":false,"realModelQualification":false,
        "nativeAuthenticationQualification":false,"sourceEffectsQualification":false})
    .to_string();
    require(
        text.len() <= 64 * 1024,
        "Complete diagnostic exceeds 64 KiB",
    )?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extra_arguments_reject_before_catalogue_and_are_never_echoed() {
        let error = run(&["https://user:secret@private.invalid/key".into()]).err();
        assert!(
            error
                .is_some_and(|error| error.message == "This fixed diagnostic accepts no arguments")
        );
    }
    #[test]
    fn projection_bounds_and_false_authority_claims_are_explicit() -> Result<(), IpcError> {
        let text = output()?;
        let value: Value = serde_json::from_str(&text).map_err(|_| IpcError::internal())?;
        for field in [
            "writeAuthorityGranted",
            "realModelQualification",
            "nativeAuthenticationQualification",
            "sourceEffectsQualification",
            "profileWalletKeychainRpcNativePromptSigningOrSendAccessed",
        ] {
            require(
                value[field] == false,
                "Diagnostic overclaimed qualification",
            )?;
        }
        Ok(())
    }
}
