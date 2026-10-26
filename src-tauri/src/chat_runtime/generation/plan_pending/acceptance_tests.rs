//! Cross-crate acceptance trajectory: inspect production-selected prompt history.
use super::*;
use ipc_types::{Message, MessageId, ToolResult, ToolResultPart};
use model_runtime::{PromptContent, PromptMessage, PromptRenderRequest};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn append(conversation: &mut Conversation, role: MessageRole, parts: Vec<MessagePart>) {
    conversation.messages.push(Message {
        id: MessageId(format!("m{}", conversation.messages.len())),
        conversation_id: conversation.id.clone(),
        role,
        parts,
        created_at: conversation.created_at.clone(),
    });
}

fn pair(
    conversation: &mut Conversation,
    calls: &[ParsedToolCall],
    outcomes: Vec<ToolOutcome>,
) -> TestResult {
    let requests = calls
        .iter()
        .map(|call| {
            serde_json::from_value::<ipc_types::ToolRequest>(serde_json::json!({
                "callId": call.call_id, "name": call.name, "arguments": call.arguments
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    append(
        conversation,
        MessageRole::Assistant,
        requests
            .iter()
            .cloned()
            .map(|request| MessagePart::ToolCall { request })
            .collect(),
    );
    for (request, outcome) in requests.into_iter().zip(outcomes) {
        append(
            conversation,
            MessageRole::Tool,
            vec![MessagePart::ToolResult {
                result: ToolResult {
                    call_id: request.call_id,
                    outcome,
                },
            }],
        );
    }
    Ok(())
}

fn completed(value: serde_json::Value) -> TestResult<ToolOutcome> {
    Ok(ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: serde_json::from_value(value)?,
        }],
    })
}

fn prompt(
    core: &AppCore,
    state: &GenerationToolState,
    conversation: &Conversation,
    all: &[PromptToolDefinition],
) -> TestResult<PromptRenderRequest> {
    let active = state.projected_tools(all);
    let profile = state.execution_profile(conversation, 0, false);
    let context = state.context(2, 1, true, profile, all, &active);
    Ok(core.assemble_trajectory_test_prompt(conversation, active, context)?)
}

fn tail(request: &PromptRenderRequest) -> TestResult<serde_json::Value> {
    let text = request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            PromptMessage::User { content } => content.iter().find_map(|part| match part {
                PromptContent::Text(text) if text.contains("<execution_state") => Some(text),
                _ => None,
            }),
            _ => None,
        })
        .ok_or("execution tail missing")?;
    let begin = text.find("<execution_state").ok_or("tail open")?;
    let body = &text[begin..];
    let start = body.find('>').ok_or("tail content")? + 1;
    let end = body[start..]
        .find("</execution_state")
        .ok_or("tail close")?
        + start;
    Ok(serde_json::from_str(&body[start..end])?)
}

#[expect(
    clippy::too_many_lines,
    reason = "keep the complete acceptance trajectory visible"
)]
fn accepted_trajectory(
    operation: &str,
    action: &str,
    all: &[PromptToolDefinition],
    bundled: bool,
) -> TestResult<PromptRenderRequest> {
    let temp = tempfile::TempDir::new()?;
    let core = AppCore::initialize(temp.path().join("app"))?;
    let mut conversation = Conversation {
        id: ipc_types::ConversationId("trajectory".into()),
        project_id: ipc_types::ProjectId("project".into()),
        title: "Trajectory".into(),
        messages: Vec::new(),
        created_at: "2026-09-11T12:00:00Z".into(),
        updated_at: "2026-09-11T12:00:00Z".into(),
    };
    append(
        &mut conversation,
        MessageRole::User,
        vec![MessagePart::Text {
            text: if operation == "x402.request" {
                "Discover available resource requirements from http://127.0.0.1:5001/test-project/us-central1/api/x402/quotes using x402. Ask me for required information and approvals before any purchase. Keep reference A-017.".into()
            } else {
                "Write A-017 to output.txt in the primary workspace.".into()
            },
        }],
    );
    let mut state = GenerationToolState {
        recall_scope: super::super::recall_scope::RecallScope::CurrentInputOnly,
        ..GenerationToolState::default()
    };
    let original = ParsedToolCall {
        call_id: "deferred-original".into(),
        name: operation.into(),
        recipient: operation.into(),
        arguments: serde_json::from_value(if operation == "x402.request" {
            serde_json::json!({"operation":"discover", "url":"http://127.0.0.1:5001/test-project/us-central1/api/x402/quotes", "reason":"Discover resource requirements for A-017"})
        } else {
            serde_json::json!({"workspace":"primary", "changes":[{"path":"output.txt", "content":"A-017", "expectedSha256":null}]})
        })?,
    };
    assert!(
        state
            .register_pending(std::slice::from_ref(&original), 0)
            .is_ok()
    );
    let deferred =
        deferred_plan_exchanges(std::slice::from_ref(&original)).map_err(|error| error.message)?;
    pair(
        &mut conversation,
        std::slice::from_ref(&original),
        deferred
            .into_iter()
            .map(|exchange| exchange.result.outcome)
            .collect(),
    )?;
    assert_eq!(
        tail(&prompt(&core, &state, &conversation, all)?)?["instruction"],
        "submit_plan_only"
    );
    assert!(
        state
            .request_recovery(
                RecoveryKind::PlanValidation,
                "initial",
                "bad-plan",
                "obsolete plan repair"
            )
            .is_ok()
    );
    let plan = ParsedToolCall {
        call_id: "accepted-plan".into(),
        name: PLAN_REQUIRED_OPERATION.into(),
        recipient: PLAN_REQUIRED_OPERATION.into(),
        arguments: serde_json::from_value(serde_json::json!({
            "contractVersion":"semantic_execution_v6", "mode":"initial", "outcome":"other", "target":"Perform requested task with A-017",
            "assumptions":[], "constraints":["Ask for required approvals"], "successCriteria":["Requested task complete"],
            "steps":[{"id":"task", "actionClass":action, "objective":"Perform requested task", "doneWhen":"Result returned", "dependsOn":[], "suggestedTools":[operation]}]
        }))?,
    };
    let mut sibling = original.clone();
    sibling.call_id = "bundled-task".into();
    let accepted =
        dispatch::admit_plan_control(&mut state, &plan, all, if bundled { 2 } else { 1 })
            .map_err(|error| error.message)?;
    state.retain_preplan_results();
    assert_eq!(accepted["accepted"], true);
    assert!(state.pending_recovery_diagnostic().is_none());
    let error = if bundled {
        let error =
            dispatch::accepted_plan_task_deferral(&mut state, std::slice::from_ref(&sibling))
                .map_err(|_| "post-acceptance correction unexpectedly exhausted")?;
        pair(
            &mut conversation,
            &[plan.clone(), sibling],
            vec![
                completed(accepted)?,
                ToolOutcome::Failed {
                    error: error.clone(),
                },
            ],
        )?;
        Some(error)
    } else {
        pair(
            &mut conversation,
            std::slice::from_ref(&plan),
            vec![completed(accepted)?],
        )?;
        None
    };
    state.observe_recovery_checkpoint();
    let next = prompt(&core, &state, &conversation, all)?;
    let capsule = tail(&next)?;
    assert!(capsule["requiredNextOperation"].is_null());
    assert_eq!(
        capsule["overallObjective"],
        "Perform requested task with A-017"
    );
    if let Some(error) = &error {
        assert_eq!(capsule["recovery"]["message"], error.message);
        assert!(error.message.contains("Plan accepted and retained"));
        assert!(error.message.contains("Do not resubmit the plan"));
        assert!(!error.message.contains("submit the accepted plan alone"));
    } else {
        assert!(capsule["recovery"].is_null());
    }
    assert!(next.messages.iter().any(|m| matches!(m, PromptMessage::Tool { call_id, content } if call_id == "accepted-plan" && content.iter().any(|p| matches!(p, PromptContent::Json(v) if v["accepted"] == true)))));
    assert!(next.messages.iter().any(|m| matches!(m, PromptMessage::Assistant { tool_calls, .. } if tool_calls.iter().any(|c| c.call_id == "deferred-original" && c.arguments == original.arguments))));
    assert!(next.tools.iter().any(|tool| tool.name == operation));
    assert!(!requires_deferral(&state, std::slice::from_ref(&original)));
    assert!(
        state
            .plan_policy
            .validate_calls(std::slice::from_ref(&original))
            .is_ok()
    );
    let revision = state.plan_policy.revision();
    let duplicate =
        dispatch::admit_plan_control(&mut state, &plan, all, 1).map_err(|error| error.message)?;
    assert_eq!(duplicate["planUnchanged"], true);
    assert_eq!(state.plan_policy.revision(), revision);
    assert_eq!(
        tail(&prompt(&core, &state, &conversation, all)?)?["recovery"]["message"],
        error
            .as_ref()
            .map_or(serde_json::Value::Null, |e| serde_json::json!(e.message))
    );
    assert!(!state.operation_ledger.x402_attempt_consumed());
    assert_eq!(state.operation_ledger.useful_result_sequence(), 0);
    assert!(
        state
            .plan_policy
            .retains_context_result("deferred-original")
    );
    Ok(next)
}

#[test]
fn accepted_plan_advances_to_task_with_original_inputs_and_no_resubmission_guidance() -> TestResult
{
    for (operation, action) in [
        ("filesystem.write_text", "workspace_write"),
        ("x402.request", "external_write"),
    ] {
        let all = if operation == "x402.request" {
            crate::chat_tools::chat_tool_definitions(&[]).map_err(|error| error.message)?
        } else {
            [
                PLAN_REQUIRED_OPERATION,
                "orchestration.recall",
                operation,
                "orchestration.search_tools",
                "orchestration.complete_stage",
            ]
            .map(|name| PromptToolDefinition {
                name: name.into(),
                description: String::new(),
                parameters: serde_json::json!({}),
            })
            .to_vec()
        };
        accepted_trajectory(operation, action, &all, true)?;
    }
    Ok(())
}

#[test]
fn standalone_acceptance_retains_accepted_exchange_after_preplan_deferral() -> TestResult {
    let all = crate::chat_tools::chat_tool_definitions(&[]).map_err(|error| error.message)?;
    accepted_trajectory("x402.request", "external_write", &all, false)?;
    Ok(())
}

#[test]
#[ignore = "requires an isolated pinned Qwen store; generates but never executes a tool call"]
fn live_native_accepted_plan_continues_to_discovery_without_resubmission() -> TestResult {
    use model_runtime::{
        DownloadCancellation, GenerationEvent, OllamaProvider, OllamaProviderConfig, OllamaSidecar,
        OllamaSidecarConfig, QwenAdapter, QwenStreamDecoder,
    };
    let all = crate::chat_tools::chat_tool_definitions(&[]).map_err(|error| error.message)?;
    let prompt = accepted_trajectory("x402.request", "external_write", &all, true)?;
    // Reject a misconfigured fixture before paying native inference latency.
    for message in &prompt.messages {
        if let PromptMessage::Assistant { tool_calls, .. } = message {
            for call in tool_calls.iter().filter(|call| call.name == "x402.request") {
                crate::chat_tools::validated_chat_tool_request(
                    &ParsedToolCall {
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        recipient: call.name.clone(),
                        arguments: call.arguments.clone(),
                    },
                    &[],
                )
                .map_err(|error| error.message)?;
            }
        }
    }
    let models =
        std::env::var_os("ZETA_LIVE_OLLAMA_MODELS").ok_or("isolated model store required")?;
    let sidecar = OllamaSidecar::new();
    let executable = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ollama-runtime/ollama");
    let endpoint = sidecar.start(
        &OllamaSidecarConfig::new(executable, models).with_guardian_executable(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug/zeta-desktop"),
        ),
    )?;
    let result = (|| -> TestResult {
        let config = OllamaProviderConfig::loopback(endpoint.address().into())?;
        let provider = OllamaProvider::new(config)?;
        let catalogue = model_runtime::bundled_catalogue()?;
        let profile = catalogue
            .models()
            .iter()
            .find(|model| model.key().id() == "qwen/qwen3.8-27b")
            .ok_or("pinned model missing")?;
        let verified = provider.verify_model(profile)?;
        let request = QwenAdapter.render_with_context(&verified, &prompt, &[], 32768, 8192)?;
        let _ = provider.render_chat_prompt(&request)?;
        let mut decoder = QwenStreamDecoder::new("post-acceptance-live")?;
        let mut calls = Vec::new();
        let mut completed = false;
        let started = std::time::Instant::now();
        provider.stream_chat_events(
            &request,
            &DownloadCancellation::new(),
            &mut decoder,
            |event| match event {
                GenerationEvent::ToolCall { call } => calls.push(call),
                GenerationEvent::Completed { .. } => completed = true,
                _ => {}
            },
        )?;
        eprintln!(
            "post_acceptance_live_ms={} completed={} calls={:?}",
            started.elapsed().as_millis(),
            completed,
            calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
        );
        assert!(completed);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "x402.request");
        assert_eq!(
            calls[0].arguments.get("operation"),
            Some(&serde_json::json!("discover"))
        );
        assert_eq!(
            calls[0].arguments.get("url"),
            Some(&serde_json::json!(
                "http://127.0.0.1:5001/test-project/us-central1/api/x402/quotes"
            ))
        );
        crate::chat_tools::validated_chat_tool_request(&calls[0], &[])
            .map_err(|error| error.message)?;
        Ok(())
    })();
    sidecar.stop()?;
    result
}
