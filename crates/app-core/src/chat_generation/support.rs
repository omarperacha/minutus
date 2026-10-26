use super::{
    AppCoreError, AssetId, Conversation, ExecutionProfile, ExecutionProfileDecision,
    ExecutionRoundBudget, InferenceError, JsonValue, MAX_PRODUCT_OUTPUT_TOKENS, Message,
    MessagePart, MessageRole, PromptContent, PromptMessage, PromptToolCall, ReasoningStrength,
    ResolvedRichInputPresentationPolicy, ToolName, ToolOutcome, ToolResultPart, working_context,
};

pub(crate) fn glimmer_ollama_image_markers(
    mut prompt: String,
    image_count: usize,
) -> Result<String, AppCoreError> {
    const GLIMMER_MARKER: &str = "<|patch|>";
    if prompt.matches(GLIMMER_MARKER).count() != image_count {
        return Err(InferenceError::ImageMarkerMismatch.into());
    }
    for index in 0..image_count {
        prompt = prompt.replacen(GLIMMER_MARKER, &format!("[img-{index}]"), 1);
    }
    Ok(prompt)
}

#[expect(
    clippy::too_many_lines,
    reason = "this closed role projection keeps unsupported content handling exhaustive at one trust boundary"
)]
pub(crate) fn prompt_message(
    message: &Message,
    asset_ids: &mut Vec<AssetId>,
    presentation: &ResolvedRichInputPresentationPolicy,
    rich_evidence: Option<&working_context::rich_evidence::RichEvidencePrompt>,
) -> Result<PromptMessage, AppCoreError> {
    match message.role {
        MessageRole::User => {
            let content = message
                .parts
                .iter()
                .map(|part| match part {
                    MessagePart::Text { text } => Ok(Some(PromptContent::Text(text.clone()))),
                    MessagePart::Asset { asset_id } => {
                        if presentation.suppresses(asset_id) || asset_ids.contains(asset_id) {
                            return Ok(Some(PromptContent::Text(
                                "Image bytes omitted by host presentation policy.".to_owned(),
                            )));
                        }
                        asset_ids.push(asset_id.clone());
                        Ok(Some(PromptContent::Image))
                    }
                    MessagePart::Activity { .. }
                    | MessagePart::Reasoning { .. }
                    | MessagePart::ToolCall { .. }
                    | MessagePart::ToolResult { .. }
                    | MessagePart::FilesystemUndo { .. }
                    | MessagePart::FilesystemWriteRecord { .. } => {
                        Err(AppCoreError::UnsupportedConversationContent)
                    }
                })
                .collect::<Result<Vec<_>, AppCoreError>>()?
                .into_iter()
                .flatten()
                .collect();
            Ok(PromptMessage::User { content })
        }
        MessageRole::Assistant => {
            let mut reasoning = None;
            let mut content = Vec::new();
            let mut tool_calls = Vec::new();
            for part in &message.parts {
                match part {
                    MessagePart::Text { text } => {
                        content.push(PromptContent::Text(text.clone()));
                    }
                    MessagePart::Reasoning { text } if reasoning.is_none() => {
                        reasoning = Some(text.clone());
                    }
                    MessagePart::Activity { .. } => {}
                    MessagePart::ToolCall { request } => {
                        tool_calls.push(PromptToolCall {
                            call_id: request.call_id.0.clone(),
                            name: tool_name(&request.name).to_owned(),
                            arguments: request
                                .arguments
                                .iter()
                                .map(|(name, value)| (name.clone(), prompt_json_value(value)))
                                .collect(),
                        });
                    }
                    MessagePart::Reasoning { .. }
                    | MessagePart::Asset { .. }
                    | MessagePart::ToolResult { .. }
                    | MessagePart::FilesystemUndo { .. }
                    | MessagePart::FilesystemWriteRecord { .. } => {
                        return Err(AppCoreError::UnsupportedConversationContent);
                    }
                }
            }
            Ok(PromptMessage::Assistant {
                reasoning,
                content,
                tool_calls,
            })
        }
        MessageRole::Tool => {
            let mut call_id = None;
            let mut content = Vec::new();
            for part in &message.parts {
                match part {
                    MessagePart::ToolResult { result } if call_id.is_none() => {
                        call_id = Some(result.call_id.0.clone());
                        match &result.outcome {
                            ToolOutcome::Completed { content: parts } => {
                                content.extend(parts.iter().map(|part| {
                                    prompt_tool_result_part(
                                        part,
                                        asset_ids,
                                        presentation,
                                        rich_evidence,
                                    )
                                }));
                            }
                            ToolOutcome::Failed { error } => {
                                content.push(PromptContent::Json(serde_json::json!({
                                    "error": error.message,
                                    "retryable": error.retryable,
                                })));
                            }
                        }
                    }
                    MessagePart::FilesystemUndo { .. }
                    | MessagePart::FilesystemWriteRecord { .. } => {}
                    _ => return Err(AppCoreError::UnsupportedConversationContent),
                }
            }
            Ok(PromptMessage::Tool {
                call_id: call_id.ok_or(AppCoreError::UnsupportedConversationContent)?,
                content,
            })
        }
    }
}

pub(crate) fn latest_user_selection_text(conversation: &Conversation) -> Option<&str> {
    conversation
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)?
        .parts
        .iter()
        .find_map(|part| match part {
            MessagePart::Text { text } => Some(text.as_str()),
            _ => None,
        })
}

pub(crate) fn prompt_tool_result_part(
    part: &ToolResultPart,
    asset_ids: &mut Vec<AssetId>,
    presentation: &ResolvedRichInputPresentationPolicy,
    _rich_evidence: Option<&working_context::rich_evidence::RichEvidencePrompt>,
) -> PromptContent {
    match part {
        ToolResultPart::Text { text } => PromptContent::Text(text.clone()),
        ToolResultPart::Json { value } => PromptContent::Json(prompt_json_value(value)),
        ToolResultPart::Asset { asset_id } => {
            if presentation.suppresses(asset_id) || asset_ids.contains(asset_id) {
                return PromptContent::Text(
                    "Image bytes omitted by host presentation policy.".to_owned(),
                );
            }
            asset_ids.push(asset_id.clone());
            PromptContent::Image
        }
    }
}

pub(crate) const fn tool_name(name: &ToolName) -> &'static str {
    match name {
        ToolName::Lookup => "lookup",
        ToolName::ComputationRun => "computation.run",
        ToolName::ComputationRunProject => "computation.run_project",
        ToolName::WebRetrieve => "web.retrieve",
        ToolName::WebSearch => "web.search",
        ToolName::X402Request => "x402.request",
        ToolName::FilesystemList => "filesystem.list",
        ToolName::FilesystemFindPaths => "filesystem.find_paths",
        ToolName::FilesystemCollectEvidence => "filesystem.collect_evidence",
        ToolName::FilesystemStat => "filesystem.stat",
        ToolName::FilesystemReadText => "filesystem.read_text",
        ToolName::FilesystemInspectImage => "filesystem.inspect_image",
        ToolName::FilesystemSearch => "filesystem.search",
        ToolName::FilesystemScan => "filesystem.scan",
        ToolName::FilesystemCreateDirectory => "filesystem.create_directory",
        ToolName::FilesystemWriteText => "filesystem.write_text",
        ToolName::FilesystemReplaceText => "filesystem.replace_text",
        ToolName::FilesystemWriteDelimitedTable => "filesystem.write_delimited_table",
        ToolName::OrchestrationTransformArtifact => "orchestration.transform_artifact",
        ToolName::OrchestrationSubmitPlan => "orchestration.submit_plan",
        ToolName::OrchestrationSubmitForecast => "orchestration.submit_forecast",
        ToolName::OrchestrationCompleteStage => "orchestration.complete_stage",
        ToolName::OrchestrationRecall => "orchestration.recall",
        ToolName::OrchestrationSearchTools => "orchestration.search_tools",
        ToolName::OrchestrationRefineStage => "orchestration.refine_stage",
        ToolName::OrchestrationToolCorrection => "orchestration.tool_correction",
        ToolName::ReadWorkspaceFile => "readWorkspaceFile",
        ToolName::ProposeWorkspaceChange => "proposeWorkspaceChange",
        ToolName::FetchUrl => "fetchUrl",
        ToolName::ProposeTransaction => "proposeTransaction",
    }
}

pub(crate) fn prompt_json_value(value: &JsonValue) -> serde_json::Value {
    match value {
        JsonValue::Null => serde_json::Value::Null,
        JsonValue::Bool(value) => serde_json::Value::Bool(*value),
        JsonValue::Number(value) => serde_json::Number::from_f64(*value)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        JsonValue::String(value) => serde_json::Value::String(value.clone()),
        JsonValue::Array(values) => {
            serde_json::Value::Array(values.iter().map(prompt_json_value).collect())
        }
        JsonValue::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(name, value)| (name.clone(), prompt_json_value(value)))
                .collect(),
        ),
    }
}
pub(crate) fn product_output_tokens(model: &model_runtime::ModelManifest) -> u32 {
    model
        .defaults()
        .output_tokens()
        .min(MAX_PRODUCT_OUTPUT_TOKENS)
}

pub(crate) const fn reasoning_strength(profile: ExecutionProfile) -> ReasoningStrength {
    match profile {
        // Fast mode historically disabled Qwen thinking. Keep that behavior
        // after giving the adapter's Low strength its actual native meaning.
        ExecutionProfile::Latency => ReasoningStrength::Disabled,
        ExecutionProfile::Balanced => ReasoningStrength::Medium,
        ExecutionProfile::Deep => ReasoningStrength::High,
    }
}

pub(crate) const fn round_reasoning_strength(
    decision: ExecutionProfileDecision,
) -> ReasoningStrength {
    reasoning_strength(decision.profile)
}

pub(crate) fn execution_round_budget(
    decision: ExecutionProfileDecision,
    maximum_output_tokens: u32,
    final_response_reserve: u64,
) -> ExecutionRoundBudget {
    let profile_output_tokens = if decision.bulk_arguments {
        maximum_output_tokens
    } else if decision.bounded_action {
        decision
            .profile
            .output_limit(maximum_output_tokens)
            .max((maximum_output_tokens / 2).max(2))
    } else {
        decision.profile.output_limit(maximum_output_tokens)
    };
    let preserved_final_output = u32::try_from(final_response_reserve)
        .unwrap_or(u32::MAX)
        .min(maximum_output_tokens);
    let output_tokens = if decision.preserves_final_answer_output() {
        profile_output_tokens.max(preserved_final_output)
    } else {
        profile_output_tokens
    };
    ExecutionRoundBudget {
        output_tokens,
        profile_output_tokens,
        reasoning_reserve_tokens: decision.profile.reasoning_reserve(maximum_output_tokens),
        final_answer_output_override: output_tokens != profile_output_tokens,
    }
}

#[cfg(test)]
mod reasoning_policy_tests {
    use super::*;

    #[test]
    fn fast_mode_disables_thinking_and_preserves_explicit_effort_overrides() {
        let mut decision = ExecutionProfileDecision {
            profile: ExecutionProfile::Latency,
            work: agent_core::ExecutionWorkKind::Evidence,
            policy_version: agent_core::EXECUTION_PROFILE_POLICY_VERSION,
            bounded_action: false,
            bulk_arguments: false,
        };
        assert_eq!(
            round_reasoning_strength(decision),
            ReasoningStrength::Disabled
        );
        decision.bounded_action = true;
        assert_eq!(
            round_reasoning_strength(decision),
            ReasoningStrength::Disabled
        );
        decision.bounded_action = false;
        decision.work = agent_core::ExecutionWorkKind::FinalResponse;
        assert_eq!(
            round_reasoning_strength(decision),
            ReasoningStrength::Disabled
        );
        decision.profile = ExecutionProfile::Deep;
        assert_eq!(round_reasoning_strength(decision), ReasoningStrength::High);
        decision.profile = ExecutionProfile::Balanced;
        assert_eq!(
            round_reasoning_strength(decision),
            ReasoningStrength::Medium
        );
    }
}

#[cfg(test)]
mod payload_budget_tests {
    use super::*;

    #[test]
    fn artifact_capacity_is_independent_of_reasoning_and_checkpoint_flags() {
        for profile in [ExecutionProfile::Latency, ExecutionProfile::Balanced] {
            for bounded_action in [false, true] {
                let decision = ExecutionProfileDecision {
                    profile,
                    work: agent_core::ExecutionWorkKind::Mutation,
                    policy_version: agent_core::EXECUTION_PROFILE_POLICY_VERSION,
                    bounded_action,
                    bulk_arguments: true,
                };
                let budget = execution_round_budget(decision, 8192, 6144);
                assert_eq!(budget.output_tokens, 8192);
                assert_eq!(
                    budget.reasoning_reserve_tokens,
                    profile.reasoning_reserve(8192)
                );
                assert!(!budget.final_answer_output_override);
                assert_eq!(
                    execution_round_budget(decision, 1024, 6144).output_tokens,
                    1024
                );
            }
        }
    }
}
