//! Text-only research must survive the completed-plan to final-answer boundary.

use super::*;
use ipc_types::{IpcError, IpcErrorCode, ToolCallId, ToolRequest, ToolResult};
use model_runtime::{QwenAdapter, VerifiedOllamaModel};

mod replay;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const NOW: &str = "2026-09-03T10:30:36Z";

fn context(progress: ToolPlanProgress) -> ToolOrchestrationContext {
    ToolOrchestrationContext::new(
        ToolOperationBudget::new(7, 5, 9, 9, 0, 2),
        false,
        ToolOrchestrationPhase::Execution,
    )
    .with_execution_state(ToolExecutionState::Completion)
    .with_phase_directive(
        ExecutionProfileDecision {
            profile: ExecutionProfile::Latency,
            work: ExecutionWorkKind::FinalResponse,
            policy_version: agent_core::EXECUTION_PROFILE_POLICY_VERSION,
            bounded_action: false,
            bulk_arguments: false,
        },
        &[],
        &[],
        &[],
        false,
        ToolPhaseScope::FinalResponseReady,
    )
    .with_plan_progress(Some(progress))
}

fn request(
    conversation: &Conversation,
    context: ToolOrchestrationContext,
) -> TestResult<PromptRenderRequest> {
    let working = working_context::derive(conversation)?;
    let (request, assets) = AppCore::build_working_chat_request(
        conversation,
        WorkingPromptAdapterContext {
            current_date: "2026-09-03",
            tools: Vec::new(),
            adapter: ModelAdapterKind::QwenNative,
            reasoning_strength: ReasoningStrength::Disabled,
        },
        WorkingPromptContext {
            working: &working,
            summary: None,
            ledger: None,
            orchestration: Some(context),
            presentation: &ResolvedRichInputPresentationPolicy::exact_for_test([]),
            rich_evidence: None,
            compact_completed_exchanges: false,
            output_token_limit: Some(6144),
        },
    )?;
    assert!(assets.is_empty());
    Ok(request)
}

fn native_json(
    model: &VerifiedOllamaModel,
    request: &PromptRenderRequest,
) -> TestResult<serde_json::Value> {
    Ok(serde_json::to_value(QwenAdapter.render_with_context(
        model,
        request,
        &[],
        32768,
        6144,
    )?)?)
}

fn append(conversation: &mut Conversation, role: MessageRole, parts: Vec<MessagePart>) {
    conversation.messages.push(Message {
        id: MessageId(format!("message-{}", conversation.messages.len())),
        conversation_id: conversation.id.clone(),
        role,
        parts,
        created_at: NOW.to_owned(),
    });
}

fn fixture() -> TestResult<(Conversation, ToolPlanProgress)> {
    let mut conversation = Conversation {
        id: ConversationId("research".to_owned()),
        project_id: ProjectId("project".to_owned()),
        title: "Research comparison".to_owned(),
        messages: Vec::new(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    };
    for text in [
        "unrelated-previous-question",
        "Compare the estimates for the last five years and cite sources.",
    ] {
        append(
            &mut conversation,
            MessageRole::User,
            vec![MessagePart::Text {
                text: text.to_owned(),
            }],
        );
    }
    for (id, outcome) in [
        (
            "search",
            ToolOutcome::Completed {
                content: vec![ToolResultPart::Json {
                    value: serde_json::from_value(serde_json::json!({
                        "untrustedSearchResults": {"items":[{"title":"Estimate A", "url":"https://example.org/a", "snippet":"Estimated 120; includes non-civilians. Attribution uncertain."}], "omittedResults":2}
                    }))?,
                }],
            },
        ),
        (
            "independent",
            ToolOutcome::Completed {
                content: vec![ToolResultPart::Text {
                    text: "Estimate B: confirmed minimum 70; https://example.org/b".to_owned(),
                }],
            },
        ),
        (
            "unresolved",
            ToolOutcome::Failed {
                error: IpcError {
                    code: IpcErrorCode::PayloadTooLarge,
                    message: "Source C unavailable; coverage incomplete".to_owned(),
                    retryable: true,
                },
            },
        ),
    ] {
        append(
            &mut conversation,
            MessageRole::Assistant,
            vec![
                MessagePart::Reasoning {
                    text: "private-synthesis-must-not-be-replayed".to_owned(),
                },
                MessagePart::ToolCall {
                    request: ToolRequest {
                        call_id: ToolCallId(id.to_owned()),
                        name: ToolName::WebSearch,
                        arguments: BTreeMap::from([(
                            "query".to_owned(),
                            JsonValue::String(id.to_owned()),
                        )]),
                    },
                },
            ],
        );
        append(
            &mut conversation,
            MessageRole::Tool,
            vec![MessagePart::ToolResult {
                result: ToolResult {
                    call_id: ToolCallId(id.to_owned()),
                    outcome,
                },
            }],
        );
    }
    Ok((conversation, completed_progress()))
}

fn completed_progress() -> ToolPlanProgress {
    let steps = [
        ("research", vec![], vec!["search".to_owned()]),
        ("independent", vec![], vec!["independent".to_owned()]),
        ("compare", vec!["research".to_owned()], vec![]),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (id, dependencies, calls))| {
        ToolPlanStepProgress::new(index + 1, ToolPlanStepStatus::ForecastComplete, Vec::new())
            .with_contract(
                id.to_owned(),
                id.to_owned(),
                Vec::new(),
                "Estimates compared with caveats".to_owned(),
                calls,
            )
            .with_dependencies(dependencies)
    })
    .collect();
    ToolPlanProgress::new(
        1,
        0,
        steps,
        None,
        ToolPlanOutcome::Answer,
        false,
        false,
        2,
        false,
        0,
    )
    .with_outcome_contract(
        "Compare estimates".to_owned(),
        vec!["Do not conflate total deaths with civilian deaths".to_owned()],
        vec!["Name sources and coverage limitations".to_owned()],
    )
    .with_retained_call_ids(["unresolved".to_owned()])
}

#[test]
fn completed_research_reaches_tool_free_native_answer_with_sources_and_limitations() -> TestResult {
    let (conversation, progress) = fixture()?;
    let request = request(&conversation, context(progress))?;
    assert!(request.tools.is_empty());
    assert_eq!(request.historical_tool_names, ["web.search"]);
    let model = super::tests::verified_model()?;
    let native = native_json(&model, &request)?;
    assert_eq!(native["think"], false);
    assert_eq!(native["truncate"], false);
    assert_eq!(native["options"]["num_ctx"], 32768);
    assert_eq!(native["options"]["num_predict"], 6144);
    let text = native.to_string();
    assert!(text.contains("provider limit of 6144 output tokens total"));
    assert!(text.contains("shared by thinking, answer text and tool calls"));
    for expected in [
        "Compare the estimates for the last five years",
        "Compare estimates",
        "Do not conflate",
        "Name sources and coverage limitations",
        "https://example.org/a",
        "https://example.org/b",
        "Attribution uncertain",
        "includes non-civilians",
        "omittedResults",
        "coverage incomplete",
    ] {
        assert!(text.contains(expected), "final answer lost {expected}");
    }
    // No explicit compaction boundary was set: prior user dialogue survives.
    assert!(text.contains("unrelated-previous-question"));
    // Active-loop reasoning remains in the native channel, never visible text.
    for message in native["messages"].as_array().ok_or("messages missing")? {
        assert!(
            !message["content"]
                .as_str()
                .unwrap_or_default()
                .contains("private-synthesis-must-not-be-replayed")
        );
    }
    assert!(text.contains("report_verified_outcome"));
    // Native rendering also validates every selected assistant-call/tool-result pair.
    assert_eq!(
        native["messages"]
            .as_array()
            .ok_or("messages missing")?
            .iter()
            .filter(|m| m["role"] == "tool")
            .count(),
        3
    );
    Ok(())
}

#[test]
fn final_answer_retains_a_producer_absent_from_plan_metadata_after_compaction() -> TestResult {
    let (mut conversation, progress) = fixture()?;
    if let Some(MessagePart::Text { text }) = conversation.messages[0].parts.first_mut() {
        text.push_str(&" older conversation ".repeat(10000));
    }
    let call_id = ToolCallId("revision-removed-producer".into());
    append(
        &mut conversation,
        MessageRole::Assistant,
        vec![MessagePart::ToolCall {
            request: ToolRequest {
                call_id: call_id.clone(),
                name: ToolName::WebRetrieve,
                arguments: BTreeMap::new(),
            },
        }],
    );
    append(
        &mut conversation,
        MessageRole::Tool,
        vec![MessagePart::ToolResult {
            result: ToolResult {
                call_id,
                outcome: ToolOutcome::Completed {
                    content: vec![ToolResultPart::Text {
                        text: "INDEPENDENT_FINAL_EVIDENCE".into(),
                    }],
                },
            },
        }],
    );
    append(
        &mut conversation,
        MessageRole::User,
        vec![MessagePart::Text {
            text: "Finish the comparison".into(),
        }],
    );
    assert!(
        working_context::derive(&conversation)?
            .message_range()
            .start
            > 0
    );
    let request = request(&conversation, context(progress))?;
    assert!(request.tools.is_empty());
    let encoded = native_json(&super::tests::verified_model()?, &request)?.to_string();
    assert!(encoded.contains("INDEPENDENT_FINAL_EVIDENCE"));
    Ok(())
}
