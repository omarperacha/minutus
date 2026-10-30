//! Closed argument decoding and semantic validation for model tool calls.

use std::collections::BTreeMap;

use ipc_types::{
    FilesystemFindPathKind, FilesystemFindPathQuery, FilesystemFindPathsRequest,
    FilesystemInspectImageRequest, FilesystemListRequest, FilesystemReadTextRequest,
    FilesystemReplaceTextRequest, FilesystemScanRequest, FilesystemSearchRequest,
    FilesystemStatRequest, FilesystemTextReplacement, FilesystemWriteTextChange,
    FilesystemWriteTextRequest, IpcError, ProjectId, ToolCallId, ToolName, ToolRequest,
    ValidateIpc, WorkspaceGrantId, WorkspacePermissionLevel,
};
use model_runtime::{ParsedToolCall, PromptToolDefinition};

use super::filesystem::ipc_json_value;
mod schema_repair;
mod semantics;

pub(crate) use schema_repair::exact_argument_repair_error;
pub(super) use semantics::{
    find_path_query_from_object, required_bool_argument, required_find_path_queries_argument,
    required_string_argument, required_string_array_argument, required_text_replacements_argument,
    required_usize_argument, required_write_changes_argument, validate_chat_tool_semantics,
};

use super::{ChatToolWorkspace, DEFAULT_FIND_PATH_DEPTH};
use super::{artifact_transform, collect_evidence, computation, web, web_search};

#[expect(
    clippy::too_many_lines,
    reason = "closed tool validation keeps contract selection and workspace authority checks exhaustive"
)]
pub(crate) fn validated_chat_tool_request(
    call: &ParsedToolCall,
    workspaces: &[ChatToolWorkspace],
) -> Result<ToolRequest, IpcError> {
    if call.recipient != call.name {
        return Err(IpcError::invalid_request(
            "The model returned an invalid tool recipient",
        ));
    }
    let Some((name, expected)) = chat_tool_contract(&call.name) else {
        return Err(IpcError::invalid_request(
            "The model requested an unavailable tool",
        ));
    };
    let arguments_valid = if call.name == "x402.request" {
        call.arguments.keys().all(|key| {
            matches!(key.as_str(), "quote" | "request")
                || expected.binary_search(&key.as_str()).is_ok()
        })
    } else if call.name == "wallet.request_transfer" {
        ["amount", "asset", "network", "recipient"]
            .iter()
            .all(|key| call.arguments.contains_key(*key))
            && call
                .arguments
                .keys()
                .all(|key| expected.binary_search(&key.as_str()).is_ok())
    } else if matches!(
        call.name.as_str(),
        "web.search" | "orchestration.complete_stage" | "orchestration.search_tools"
    ) {
        required_contract_arguments(&call.name)
            .iter()
            .all(|key| call.arguments.contains_key(*key))
            && call
                .arguments
                .keys()
                .all(|key| expected.binary_search(&key.as_str()).is_ok())
    } else if call.name == "filesystem.write_delimited_table" {
        expected.iter().all(|key| call.arguments.contains_key(*key))
            && call
                .arguments
                .keys()
                .all(|key| key == "evidence" || expected.binary_search(&key.as_str()).is_ok())
    } else {
        call.arguments
            .keys()
            .map(String::as_str)
            .eq(expected.iter().copied())
    };
    if !arguments_valid {
        return Err(contract_argument_error(call, expected));
    }
    if matches!(
        name,
        ToolName::OrchestrationSubmitPlan
            | ToolName::OrchestrationSubmitForecast
            | ToolName::OrchestrationCompleteStage
            | ToolName::OrchestrationSearchTools
            | ToolName::OrchestrationRecall
            | ToolName::OrchestrationRefineStage
            | ToolName::ComputationRun
            | ToolName::WebRetrieve
            | ToolName::WebSearch
            | ToolName::X402Request
            | ToolName::WalletRequestTransfer
            | ToolName::ContractInspect
            | ToolName::ContractRead
            | ToolName::ContractRequestWorkflow
    ) {
        if name == ToolName::OrchestrationRecall {
            super::recall::arguments(call)?;
        } else if name == ToolName::ComputationRun {
            computation::validate_call(call)?;
        } else if name == ToolName::WebRetrieve {
            web::validate_call(call)?;
        } else if name == ToolName::WebSearch {
            web_search::validate_call(call)?;
        } else if matches!(name, ToolName::ContractInspect | ToolName::ContractRead) {
            super::contracts::validate_call(call)?;
        } else if name == ToolName::ContractRequestWorkflow {
            super::contracts::validate_write_call(call)?;
        } else if name == ToolName::WalletRequestTransfer {
            super::wallet_transfer::validate_call(call)?;
        } else if name == ToolName::X402Request {
            super::x402::validate_call(call)?;
        }
        let request = ToolRequest {
            call_id: ToolCallId(call.call_id.clone()),
            name,
            arguments: call
                .arguments
                .iter()
                .map(|(name, value)| Ok((name.clone(), ipc_json_value(value)?)))
                .collect::<Result<_, IpcError>>()?,
        };
        request.validate()?;
        return Ok(request);
    }
    let workspace = required_string_argument(&call.arguments, "workspace")?;
    let selected_workspace = workspaces
        .iter()
        .find(|candidate| candidate.alias == workspace)
        .ok_or_else(|| {
            IpcError::scope_correction(
                "The requested project workspace is outside the active authorized scope",
            )
        })?;
    if matches!(
        name,
        ToolName::FilesystemCreateDirectory
            | ToolName::FilesystemWriteText
            | ToolName::FilesystemReplaceText
            | ToolName::FilesystemWriteDelimitedTable
            | ToolName::OrchestrationTransformArtifact
    ) && selected_workspace.permission_level != WorkspacePermissionLevel::ReadAndWrite
    {
        return Err(IpcError::permission_denied(
            "The requested project workspace is read only; the planned mutation was not authorized",
        ));
    }
    validate_chat_tool_semantics(&name, &call.arguments)?;
    let request = ToolRequest {
        call_id: ToolCallId(call.call_id.clone()),
        name,
        arguments: call
            .arguments
            .iter()
            .map(|(name, value)| Ok((name.clone(), ipc_json_value(value)?)))
            .collect::<Result<_, _>>()?,
    };
    request.validate()?;
    Ok(request)
}

fn contract_argument_error(call: &ParsedToolCall, expected: &[&str]) -> IpcError {
    let explicitly_required = required_contract_arguments(&call.name);
    let required = if explicitly_required.is_empty() {
        expected
    } else {
        explicitly_required
    };
    let missing = required
        .iter()
        .filter(|name| !call.arguments.contains_key(**name))
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>();
    let unexpected = call
        .arguments
        .keys()
        .filter(|name| !expected.contains(&name.as_str()))
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>();
    let mut reasons = Vec::new();
    if !missing.is_empty() {
        reasons.push(format!("missing required field(s): {}", missing.join(", ")));
    }
    if !unexpected.is_empty() {
        reasons.push(format!("unexpected field(s): {}", unexpected.join(", ")));
    }
    let reason = if reasons.is_empty() {
        "fields must match the advertised closed schema".to_owned()
    } else {
        reasons.join("; ")
    };
    IpcError::invalid_request(format!(
        "The model returned invalid {} arguments: {reason}",
        call.name
    ))
}

fn required_contract_arguments(operation: &str) -> &'static [&'static str] {
    match operation {
        "web.search" => &["query"],
        _ => &[],
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the closed name-to-argument contract is intentionally centralized and exhaustive"
)]
pub(super) fn chat_tool_contract(name: &str) -> Option<(ToolName, &'static [&'static str])> {
    let contract = match name {
        "filesystem.list" => (ToolName::FilesystemList, &["path", "workspace"][..]),
        "filesystem.find_paths" => (ToolName::FilesystemFindPaths, &["queries", "workspace"][..]),
        "filesystem.collect_evidence" => (
            ToolName::FilesystemCollectEvidence,
            &["items", "workspace"][..],
        ),
        "filesystem.stat" => (ToolName::FilesystemStat, &["path", "workspace"][..]),
        "filesystem.read_text" => (ToolName::FilesystemReadText, &["path", "workspace"][..]),
        "filesystem.inspect_image" => {
            (ToolName::FilesystemInspectImage, &["path", "workspace"][..])
        }
        "filesystem.search" => (
            ToolName::FilesystemSearch,
            &["path", "query", "workspace"][..],
        ),
        "filesystem.scan" => (
            ToolName::FilesystemScan,
            &[
                "excludeExtensions",
                "minimumLineCount",
                "path",
                "recursive",
                "workspace",
            ][..],
        ),
        "filesystem.create_directory" => (
            ToolName::FilesystemCreateDirectory,
            &["path", "workspace"][..],
        ),
        "filesystem.write_text" => (ToolName::FilesystemWriteText, &["changes", "workspace"][..]),
        "filesystem.replace_text" => (
            ToolName::FilesystemReplaceText,
            &["expectedSha256", "path", "replacements", "workspace"][..],
        ),
        "filesystem.write_delimited_table" => (
            ToolName::FilesystemWriteDelimitedTable,
            &[
                "columns",
                "delimiter",
                "expectedSha256",
                "path",
                "rows",
                "workspace",
            ][..],
        ),
        "orchestration.transform_artifact" => (
            ToolName::OrchestrationTransformArtifact,
            &[
                "evidence",
                "expectedSha256",
                "path",
                "representation",
                "workspace",
            ][..],
        ),
        "computation.run" => (ToolName::ComputationRun, &["output", "source"][..]),
        "computation.run_project" => (
            ToolName::ComputationRunProject,
            &["inputs", "output", "source", "workspace"][..],
        ),
        "web.retrieve" => (ToolName::WebRetrieve, &["method", "url"][..]),
        "web.search" => (
            ToolName::WebSearch,
            &["count", "country", "freshness", "query", "searchLanguage"][..],
        ),
        "contract.inspect" => (
            ToolName::ContractInspect,
            &["block", "network", "target"][..],
        ),
        "contract.request_workflow" => {
            (ToolName::ContractRequestWorkflow, &["network", "steps"][..])
        }
        "contract.read" => (
            ToolName::ContractRead,
            &[
                "arguments",
                "block",
                "caller",
                "function",
                "network",
                "outputs",
                "target",
            ][..],
        ),
        "wallet.request_transfer" => (
            ToolName::WalletRequestTransfer,
            &["amount", "asset", "network", "reason", "recipient"][..],
        ),
        "x402.request" => (
            ToolName::X402Request,
            &["input", "operation", "reason", "resourceRef", "url"][..],
        ),
        "orchestration.submit_plan" => (
            ToolName::OrchestrationSubmitPlan,
            &[
                "assumptions",
                "constraints",
                "contractVersion",
                "mode",
                "outcome",
                "steps",
                "successCriteria",
                "target",
            ][..],
        ),
        "orchestration.submit_forecast" => (
            ToolName::OrchestrationSubmitForecast,
            &["contractVersion", "entries", "mode"][..],
        ),
        "orchestration.complete_stage" => (ToolName::OrchestrationCompleteStage, &[][..]),
        "orchestration.recall" => (
            ToolName::OrchestrationRecall,
            &["limit", "offset", "reference"][..],
        ),
        "orchestration.search_tools" => (ToolName::OrchestrationSearchTools, &["queries"][..]),
        "orchestration.refine_stage" => (
            ToolName::OrchestrationRefineStage,
            &[
                "allowedTools",
                "evidenceGap",
                "id",
                "maxCalls",
                "parentStageId",
                "purpose",
            ][..],
        ),
        _ => return None,
    };
    Some(contract)
}

/// Whether a registered model-facing tool represents task work that may appear
/// in a provisional execution forecast.
///
/// Orchestration controls declare or manage the forecast itself. They are
/// executable host calls, but forecasting them as task work would create a
/// recursive plan step and distort progress and allowance accounting.
pub(crate) fn is_forecastable_chat_tool(name: &str) -> bool {
    chat_tool_contract(name).is_some_and(|(tool, _)| {
        !matches!(
            tool,
            ToolName::OrchestrationSubmitPlan
                | ToolName::OrchestrationSubmitForecast
                | ToolName::OrchestrationCompleteStage
                | ToolName::OrchestrationSearchTools
                | ToolName::OrchestrationRecall
                | ToolName::OrchestrationRefineStage
        )
    })
}

#[cfg(test)]
mod tests;
