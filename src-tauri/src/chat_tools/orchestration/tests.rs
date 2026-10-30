use std::collections::BTreeMap;

use super::*;
use ipc_types::{ToolCallId, ToolName, ToolRequest, WorkspaceGrantId, WorkspacePermissionLevel};

fn prepared(id: &str, name: &str, path: &str) -> PreparedChatTool {
    let call = ParsedToolCall {
        call_id: id.to_owned(),
        recipient: name.to_owned(),
        name: name.to_owned(),
        arguments: BTreeMap::from([
            ("path".to_owned(), serde_json::json!(path)),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    PreparedChatTool {
        request: ToolRequest {
            call_id: ToolCallId(id.to_owned()),
            name: ToolName::FilesystemReadText,
            arguments: BTreeMap::new(),
        },
        workspace: Some(ChatToolWorkspace {
            alias: "primary".to_owned(),
            display_name: "project".to_owned(),
            grant_id: WorkspaceGrantId("grant".to_owned()),
            permission_level: WorkspacePermissionLevel::ReadOnly,
        }),
        activity: crate::chat_runtime::tool_activity(id, name),
        call,
    }
}

#[test]
fn aggregate_reads_are_prioritized_and_independent_reads_share_a_wave() {
    let calls = vec![
        prepared("read", "filesystem.read_text", "src/lib.rs"),
        prepared("scan", "filesystem.scan", "."),
        prepared("find", "filesystem.find_paths", "."),
        prepared("stat", "filesystem.stat", "Cargo.toml"),
    ];
    let plan = plan_batch(&calls);
    assert!(plan.concurrent_reads);
    assert_eq!(plan.execution_order, vec![1, 2, 0, 3]);
    assert_eq!(plan.duplicate_of, vec![None, None, None, None]);
}

#[test]
fn pre_execution_plan_deviation_returns_a_non_executing_revision_steer() -> Result<(), IpcError> {
    let call = ParsedToolCall {
        call_id: "drift".to_owned(),
        recipient: "filesystem.read_text".to_owned(),
        name: "filesystem.read_text".to_owned(),
        arguments: BTreeMap::from([
            ("path".to_owned(), serde_json::json!("example.csv")),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let prepared = prepare_invalid_tool_correction(&call)?;
    let error = IpcError::invalid_request(
        "The evidence operation `filesystem.read_text` is outside the accepted unfinished plan and was not executed. Continue with filesystem.collect_evidence or submit mode=revise first",
    );
    let exchange = plan_deviation_correction_exchange(&prepared, &error);

    assert_eq!(
        exchange.request.name,
        ipc_types::ToolName::FilesystemReadText
    );
    let ToolOutcome::Failed { error } = exchange.result.outcome else {
        return Err(IpcError::internal());
    };
    assert!(error.message.contains("was not executed"));
    assert!(error.message.contains("mode=revise"));
    assert!(!error.message.contains("corrected arguments"));
    assert_eq!(exchange.activity.status, TaskActivityStatus::Partial);
    assert!(exchange.undo_changes.is_empty());
    Ok(())
}

#[test]
fn equivalent_reads_execute_once_and_mutations_disable_parallelism() {
    let first = prepared("first", "filesystem.read_text", "src/lib.rs");
    let mut duplicate = first.clone();
    duplicate.call.call_id = "duplicate".to_owned();
    duplicate.request.call_id = ToolCallId("duplicate".to_owned());
    let plan = plan_batch(&[first, duplicate]);
    assert_eq!(plan.execution_order, vec![0]);
    assert_eq!(plan.duplicate_of, vec![None, Some(0)]);

    let plan = plan_batch(&[prepared("write", "filesystem.write_text", "src/lib.rs")]);
    assert!(!plan.concurrent_reads);

    let transform = prepared(
        "transform",
        "orchestration.transform_artifact",
        "outputs/data.csv",
    );
    let plan = plan_batch(std::slice::from_ref(&transform));
    assert!(!plan.concurrent_reads);
    assert_eq!(estimated_output_bytes(&transform.call), 1024 * 1024);
}

#[test]
fn web_calls_repeat_but_x402_consumes_one_request_local_workflow() {
    let first = prepared("web-first", "web.retrieve", "unused");
    let mut repeated = first.clone();
    repeated.call.call_id = "web-repeated".to_owned();
    repeated.request.call_id = ToolCallId("web-repeated".to_owned());
    let plan = plan_batch(&[first, repeated]);
    assert!(!plan.concurrent_reads);
    assert_eq!(plan.execution_order, vec![0, 1]);
    assert_eq!(plan.duplicate_of, vec![None, None]);

    let first = prepared("x402-first", "x402.request", "unused");
    let mut repeated = first.clone();
    repeated.call.call_id = "x402-repeated".to_owned();
    repeated
        .call
        .arguments
        .insert("path".to_owned(), serde_json::json!("different"));
    repeated.request.call_id = ToolCallId("x402-repeated".to_owned());
    let plan = plan_batch(&[first, repeated]);
    assert!(!plan.concurrent_reads);
    assert_eq!(plan.execution_order, vec![0]);
    assert_eq!(plan.duplicate_of, vec![None, Some(0)]);
}

#[test]
fn web_calls_consume_the_shared_call_and_output_budgets() {
    let calls = vec![
        prepared("web-one", "web.retrieve", "unused"),
        prepared("web-two", "web.retrieve", "unused"),
    ];
    let plan = plan_batch(&calls);
    assert_eq!(estimated_output_bytes(&calls[0].call), 64 * 1024);
    assert_eq!(
        admit_execution_order(&plan.execution_order, &calls, 1, 64 * 1024),
        vec![0]
    );
    assert!(admit_execution_order(&plan.execution_order, &calls, 2, 64 * 1024 - 1).is_empty());
}

#[test]
fn retry_backoff_is_bounded_and_only_typed_internal_failures_are_transient() {
    assert_eq!(MAX_TRANSIENT_RETRIES, 2);
    assert_eq!(RETRY_BACKOFF_MS, 25);
    let typed = IpcError {
        code: IpcErrorCode::Internal,
        message: "Temporary local contention".to_owned(),
        retryable: true,
    };
    let untyped = IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: "Bad arguments".to_owned(),
        retryable: true,
    };
    assert!(typed.retryable && typed.code == IpcErrorCode::Internal);
    assert!(!(untyped.retryable && untyped.code == IpcErrorCode::Internal));
}

#[test]
fn call_and_output_budgets_are_applied_before_execution() {
    let calls = vec![
        prepared("scan", "filesystem.scan", "."),
        prepared("read", "filesystem.read_text", "src/lib.rs"),
        prepared("stat", "filesystem.stat", "Cargo.toml"),
    ];
    let plan = plan_batch(&calls);
    assert_eq!(
        admit_execution_order(&plan.execution_order, &calls, 2, 132 * 1024),
        vec![0, 2]
    );
    assert!(admit_execution_order(&plan.execution_order, &calls, 8, 4_095).is_empty());
}

#[test]
fn scheduling_deferrals_and_withheld_reads_never_poison_exact_retry() {
    let original = prepared("read-review", "filesystem.read_text", "REVIEW.md");
    let retry = prepared("retry-review", "filesystem.read_text", "REVIEW.md");
    for disposition in [
        ScheduledReadDisposition::Deferred,
        ScheduledReadDisposition::OutputWithheld,
    ] {
        let exchange = partial_exchange(&original, disposition);
        assert_eq!(exchange.activity.status, TaskActivityStatus::Partial);
        assert!(
            matches!(&exchange.result.outcome, ToolOutcome::Failed { error }
            if error.retryable && error.message.contains("separately"))
        );
        let mut ledger = OperationLedger::default();
        ledger.record_results(std::slice::from_ref(&original.call), &[Ok(exchange)]);
        assert_eq!(ledger.useful_result_sequence(), 0);
        assert!(ledger.reusable_exchange(&retry, None).is_none());
        let plan = plan_batch(std::slice::from_ref(&retry));
        assert_eq!(
            admit_execution_order(
                &plan.execution_order,
                std::slice::from_ref(&retry),
                1,
                usize::MAX
            ),
            [0]
        );
    }
    let mut ledger = OperationLedger::default();
    ledger.record_results(
        std::slice::from_ref(&original.call),
        &[Ok(cancelled_exchange(
            &original,
            "Cancelled before execution",
        ))],
    );
    assert_eq!(ledger.useful_result_sequence(), 0);
    assert!(ledger.reusable_exchange(&retry, None).is_none());
}

#[test]
fn partial_host_results_are_not_reused_even_with_a_non_retryable_error() {
    let original = prepared("read-review", "filesystem.read_text", "REVIEW.md");
    let mut ledger = OperationLedger::default();
    ledger.record_results(
        std::slice::from_ref(&original.call),
        &[Ok(failed_exchange(
            &original,
            IpcError::payload_too_large("Old scheduler result"),
            TaskActivityStatus::Partial,
            "Not executed",
        ))],
    );
    assert_eq!(ledger.useful_result_sequence(), 0);
    assert!(ledger.reusable_exchange(&original, None).is_none());
}

#[test]
fn result_retention_distinguishes_not_run_from_withheld_and_preserves_effects()
-> Result<(), IpcError> {
    let read = prepared("review", "filesystem.read_text", "REVIEW.md");
    let mut references = PathReferenceLedger::default();
    let mut bytes = 0;
    let deferred = retain_scheduled_result(&read, None, &mut references, &mut bytes, 0, false)?;
    assert!(
        matches!(&deferred.result.outcome, ToolOutcome::Failed { error } if error.message.starts_with("Not executed"))
    );
    let mut ledger = OperationLedger::default();
    ledger.record_results(std::slice::from_ref(&read.call), &[Ok(deferred)]);
    assert!(ledger.reusable_exchange(&read, None).is_none());

    let mut executed = failed_exchange(
        &read,
        IpcError::internal(),
        TaskActivityStatus::Completed,
        "Read completed",
    );
    executed.result.outcome = ToolOutcome::Completed {
        content: Vec::new(),
    };
    let withheld = retain_scheduled_result(
        &read,
        Some(Ok(executed.clone())),
        &mut references,
        &mut bytes,
        0,
        false,
    )?;
    assert!(
        matches!(&withheld.result.outcome, ToolOutcome::Failed { error } if error.message.starts_with("The read was executed"))
    );
    ledger.record_results(std::slice::from_ref(&read.call), &[Ok(withheld)]);
    assert_eq!(ledger.useful_result_sequence(), 0);
    assert!(ledger.reusable_exchange(&read, None).is_none());
    let retained = retain_scheduled_result(
        &read,
        Some(Ok(executed.clone())),
        &mut references,
        &mut bytes,
        4096,
        false,
    )?;
    assert!(matches!(
        retained.result.outcome,
        ToolOutcome::Completed { .. }
    ));
    assert!(bytes > 0 && bytes <= 4096);

    let write = prepared("write", "filesystem.write_text", "output.csv");
    executed.request.name = ToolName::FilesystemWriteText;
    let effect = retain_scheduled_result(
        &write,
        Some(Ok(executed)),
        &mut references,
        &mut bytes,
        0,
        false,
    )?;
    assert!(matches!(
        effect.result.outcome,
        ToolOutcome::Completed { .. }
    ));
    Ok(())
}

#[test]
fn cancellation_stops_new_waves_and_a_new_batch_never_reuses_stale_state() {
    let cancelled = AtomicBool::new(true);
    assert!(should_stop(
        &cancelled,
        Instant::now(),
        Duration::from_secs(1)
    ));

    let calls = vec![prepared(
        "read-after-revision-change",
        "filesystem.read_text",
        "src/lib.rs",
    )];
    let new_plan = plan_batch(&calls);
    assert_eq!(new_plan.execution_order, vec![0]);
    assert_eq!(new_plan.duplicate_of, vec![None]);
}

#[test]
fn inactive_malformed_tool_is_returned_as_an_argument_discarding_correction()
-> Result<(), Box<dyn std::error::Error>> {
    let call = ParsedToolCall {
        call_id: "stale-find".to_owned(),
        recipient: "filesystem.find_paths".to_owned(),
        name: "filesystem.find_paths".to_owned(),
        arguments: BTreeMap::from([(
            "sensitiveMalformedArgument".to_owned(),
            serde_json::json!("must-not-be-persisted"),
        )]),
    };
    let available = vec!["orchestration.submit_plan".to_owned()];
    let tool = prepare_inactive_tool_correction(&call, &available)
        .map_err(|error| std::io::Error::other(error.message))?;
    assert!(!format!("{:?}", tool.request.arguments).contains("must-not-be-persisted"));
    let exchange = tool_availability_correction_exchange(&tool, &available);
    let ToolOutcome::Failed { error } = exchange.result.outcome else {
        return Err("phase correction unexpectedly completed".into());
    };
    assert_eq!(error.code, IpcErrorCode::InvalidRequest);
    assert!(error.retryable);
    assert!(error.message.contains("orchestration.submit_plan"));
    assert_eq!(exchange.request.name, ToolName::OrchestrationToolCorrection);
    assert!(exchange.undo_changes.is_empty());
    assert!(exchange.inspected_asset_id.is_none());
    assert_eq!(exchange.activity.status, TaskActivityStatus::Partial);
    assert_eq!(
        exchange.activity.detail.as_deref(),
        Some("Operation was not executed")
    );
    Ok(())
}

#[test]
fn unavailable_batch_does_not_mislabel_its_available_sibling()
-> Result<(), Box<dyn std::error::Error>> {
    let available = vec!["web.retrieve".to_owned(), "x402.request".to_owned()];
    for (name, expected) in [
        (
            "web.retrieve",
            "is available, but this batch was not executed",
        ),
        (
            "orchestration.search_tools",
            "is unavailable in this projection",
        ),
    ] {
        let call = ParsedToolCall {
            call_id: format!("batch-{name}"),
            recipient: name.to_owned(),
            name: name.to_owned(),
            arguments: BTreeMap::new(),
        };
        let tool = prepare_inactive_tool_correction(&call, &available)
            .map_err(|error| std::io::Error::other(error.message))?;
        let exchange = tool_availability_correction_exchange(&tool, &available);
        let ToolOutcome::Failed { error } = exchange.result.outcome else {
            return Err("batch correction unexpectedly completed".into());
        };
        assert!(error.message.contains(expected), "{}", error.message);
        assert_eq!(exchange.request.call_id.0, call.call_id);
        assert_eq!(
            exchange.request.arguments.get("requestedTool"),
            Some(&ipc_types::JsonValue::String(name.to_owned()))
        );
        assert!(!exchange.request.arguments.contains_key("unavailableTool"));
        assert!(exchange.undo_changes.is_empty());
    }
    Ok(())
}

#[test]
fn final_response_correction_discards_stale_arguments_and_requests_plain_text()
-> Result<(), Box<dyn std::error::Error>> {
    for arguments in [
        BTreeMap::from([("stage".to_owned(), serde_json::json!("write_csv"))]),
        BTreeMap::from([
            ("arguments".to_owned(), serde_json::json!("{}")),
            ("stageId".to_owned(), serde_json::json!("write_csv")),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    ] {
        let call = ParsedToolCall {
            call_id: "stale-final-control".to_owned(),
            recipient: "orchestration.complete_stage".to_owned(),
            name: "orchestration.complete_stage".to_owned(),
            arguments,
        };
        let tool = prepare_inactive_tool_correction(&call, &[])
            .map_err(|error| std::io::Error::other(error.message))?;
        assert!(!format!("{:?}", tool.request.arguments).contains("write_csv"));

        let exchange = final_response_correction_exchange(&tool);
        let ToolOutcome::Failed { error } = exchange.result.outcome else {
            return Err("final-response correction unexpectedly completed".into());
        };
        assert!(error.retryable);
        assert!(
            error
                .message
                .contains("Respond to the user now in plain text")
        );
        assert!(
            error
                .message
                .contains("tool surface is intentionally closed")
        );
        assert!(!error.message.contains("currently advertised tools"));
        assert!(!error.message.contains("forecast"));
        assert!(!error.message.contains("availableTools"));
        assert_eq!(
            exchange.activity.detail.as_deref(),
            Some("Minutus is preparing the verified final response")
        );
    }
    Ok(())
}

#[test]
fn admission_failures_preserve_typed_actionable_model_corrections()
-> Result<(), Box<dyn std::error::Error>> {
    let call = ParsedToolCall {
        call_id: "typed-correction".to_owned(),
        recipient: "filesystem.read_text".to_owned(),
        name: "filesystem.read_text".to_owned(),
        arguments: BTreeMap::from([
            ("path".to_owned(), serde_json::json!(42)),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let prepared = prepare_invalid_tool_correction(&call)
        .map_err(|error| std::io::Error::other(error.message))?;
    let cases = [
        (
            IpcError::permission_denied("write access was not granted"),
            IpcErrorCode::PermissionDenied,
            false,
            "ask the user",
        ),
        (
            IpcError::scope_correction("workspace is not active"),
            IpcErrorCode::ScopeCorrection,
            true,
            "advertised workspace/resource",
        ),
        (
            IpcError::plan_mismatch("dependency is incomplete"),
            IpcErrorCode::PlanMismatch,
            true,
            "current step",
        ),
    ];

    for (source, expected_code, retryable, actionable_choice) in cases {
        let exchange = tool_argument_correction_exchange(&prepared, &source);
        let ToolOutcome::Failed { error } = exchange.result.outcome else {
            return Err("typed correction unexpectedly completed".into());
        };
        assert_eq!(error.code, expected_code);
        assert_eq!(error.retryable, retryable);
        assert!(error.message.contains(actionable_choice));
    }
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one vertical repair test asserts the complete correction sequence"
)]
fn invalid_tool_arguments_receive_one_safe_concrete_correction()
-> Result<(), Box<dyn std::error::Error>> {
    let workspaces = vec![ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "project".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadOnly,
    }];
    let call = ParsedToolCall {
        call_id: "invalid-collector".to_owned(),
        recipient: "filesystem.collect_evidence".to_owned(),
        name: "filesystem.collect_evidence".to_owned(),
        arguments: BTreeMap::from([
            (
                "items".to_owned(),
                serde_json::json!([
                    {"id":"all_csvs","extensions":["csv"],"requirement":"Locate reconstructed tables","operation":"find_matches"},
                    {"id":"monthly_dir","path":"monthly cashflow","scope":"directory","requirement":"Locate monthly source files","operation":"find_matches"}
                ]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    let Err(validation_error) = super::super::validated_chat_tool_request(&call, &workspaces)
    else {
        return Err("directory-only evidence selector was accepted".into());
    };
    assert!(invalid_tool_batch_repairable(std::slice::from_ref(&call)));
    let prepared = prepare_invalid_tool_correction(&call)
        .map_err(|error| std::io::Error::other(error.message))?;
    let exchange = tool_argument_correction_exchange(&prepared, &validation_error);
    let ToolOutcome::Failed { error } = exchange.result.outcome else {
        return Err("tool correction unexpectedly completed".into());
    };
    assert!(error.retryable);
    assert!(error.message.contains("items[1]"));
    assert!(error.message.contains("monthly_dir"));
    assert!(error.message.contains("nameContains"));
    assert!(error.message.contains("at least one extension"));
    assert!(error.message.contains("items, workspace"));
    assert!(exchange.undo_changes.is_empty());
    assert!(exchange.workspace_grant_id.is_none());

    let read = ParsedToolCall {
        call_id: "valid-read".to_owned(),
        recipient: "filesystem.read_text".to_owned(),
        name: "filesystem.read_text".to_owned(),
        arguments: BTreeMap::from([
            ("path".to_owned(), serde_json::json!("manifest.json")),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert!(mixed_read_only_invalid_batch_repairable(
        &[read.clone(), call.clone()],
        1,
    ));
    let plan = ParsedToolCall {
        call_id: "plan".to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::new(),
    };
    assert!(plan_prefixed_invalid_operation_batch_repairable(
        &[plan.clone(), call.clone()],
        1,
    ));
    assert!(!plan_prefixed_invalid_operation_batch_repairable(
        &[plan.clone(), call.clone(), call.clone()],
        1,
    ));
    let mut mutation = read;
    mutation.name = "filesystem.write_text".to_owned();
    mutation.recipient = mutation.name.clone();
    assert!(!mixed_read_only_invalid_batch_repairable(
        &[mutation, call.clone()],
        1,
    ));
    let mut mutation = call.clone();
    mutation.name = "filesystem.write_text".to_owned();
    mutation.recipient = mutation.name.clone();
    assert!(plan_prefixed_invalid_operation_batch_repairable(
        &[plan, mutation],
        1,
    ));

    let invalid_stat = ParsedToolCall {
        call_id: "invalid-stat".to_owned(),
        recipient: "filesystem.stat".to_owned(),
        name: "filesystem.stat".to_owned(),
        arguments: BTreeMap::from([
            ("path".to_owned(), serde_json::json!(42)),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]),
    };
    assert!(super::super::validated_chat_tool_request(&invalid_stat, &workspaces).is_err());
    assert!(invalid_tool_batch_repairable(std::slice::from_ref(
        &invalid_stat
    )));
    let prepared = prepare_invalid_tool_correction(&invalid_stat)
        .map_err(|error| std::io::Error::other(error.message))?;
    let exchange = tool_argument_correction_exchange(
        &prepared,
        &IpcError::invalid_request("path must be a string"),
    );
    let ToolOutcome::Failed { error } = exchange.result.outcome else {
        return Err("stat correction unexpectedly completed".into());
    };
    assert!(error.message.contains("filesystem.stat"));
    assert!(error.message.contains("path, workspace"));

    let mut unknown = call;
    unknown.name = "filesystem.unknown".to_owned();
    unknown.recipient = "filesystem.unknown".to_owned();
    unknown.arguments.insert(
        "privateArgument".to_owned(),
        serde_json::json!("must-not-be-persisted"),
    );
    assert!(!invalid_tool_batch_repairable(std::slice::from_ref(
        &unknown
    )));
    assert!(unknown_tool_batch_repairable(std::slice::from_ref(
        &unknown
    )));
    let available = vec![
        "orchestration.submit_plan".to_owned(),
        "filesystem.write_text".to_owned(),
    ];
    let prepared = prepare_unknown_tool_correction(&unknown, &available)
        .map_err(|error| std::io::Error::other(error.message))?;
    assert_eq!(
        prepared.request.name,
        ipc_types::ToolName::OrchestrationToolCorrection
    );
    assert!(!format!("{:?}", prepared.request.arguments).contains("must-not-be-persisted"));
    let exchange = tool_availability_correction_exchange(&prepared, &available);
    let ToolOutcome::Failed { error } = exchange.result.outcome else {
        return Err("unknown-tool correction unexpectedly completed".into());
    };
    assert!(error.message.contains("currently advertised tools"));
    assert!(error.message.contains("orchestration.submit_plan"));
    assert!(
        error
            .message
            .contains("If orchestration.search_tools is advertised")
    );
    assert!(
        error
            .message
            .contains("legacy forecast may be revised only when its control is advertised")
    );
    assert!(error.message.contains("Ask the user only when"));

    let mut unsafe_recipient = unknown.clone();
    unsafe_recipient.recipient = "filesystem.different".to_owned();
    assert!(!unknown_tool_batch_repairable(&[unsafe_recipient]));
    assert!(unknown_tool_batch_repairable(&[unknown.clone(), unknown]));
    Ok(())
}

#[test]
fn duplicate_contract_observations_in_one_batch_are_each_fetched() -> Result<(), IpcError> {
    for name in ["contract.inspect", "contract.read"] {
        let value = if name == "contract.inspect" {
            serde_json::json!({"block":"latest","network":"baseSepolia","target":"0x1111111111111111111111111111111111111111"})
        } else {
            serde_json::json!({"block":"latest","network":"baseSepolia","target":"0x1111111111111111111111111111111111111111","caller":null,"function":"f()","arguments":[],"outputs":[]})
        };
        let mut call = ParsedToolCall {
            call_id: "first".into(),
            recipient: name.into(),
            name: name.into(),
            arguments: serde_json::from_value(value).unwrap_or_default(),
        };
        let first = prepare_chat_tool(&[], &call)?;
        call.call_id = "second".into();
        let second = prepare_chat_tool(&[], &call)?;
        let batch = plan_batch(&[first, second]);
        assert_eq!(batch.execution_order, vec![0, 1]);
        assert_eq!(batch.duplicate_of, vec![None, None]);
    }
    Ok(())
}

fn prepared_contract(name: &str) -> Result<PreparedChatTool, IpcError> {
    let arguments = match name {
        "contract.inspect" => {
            serde_json::json!({"block":"latest","network":"baseSepolia","target":"0x1111111111111111111111111111111111111111"})
        }
        "contract.read" => {
            serde_json::json!({"block":"latest","network":"baseSepolia","target":"0x1111111111111111111111111111111111111111","caller":null,"function":"allowance(address,address)","arguments":["0x1111111111111111111111111111111111111111","0x2222222222222222222222222222222222222222"],"outputs":["uint256"]})
        }
        "contract.request_workflow" => {
            serde_json::json!({"network":"baseSepolia","steps":[{"target":"0x1111111111111111111111111111111111111111","function":"approve(address,uint256)","arguments":["0x2222222222222222222222222222222222222222","7"],"valueWei":"0","deploymentTransaction":format!("0x{}","3".repeat(64))}]})
        }
        _ => return Err(IpcError::internal()),
    };
    let call = ParsedToolCall {
        call_id: name.into(),
        recipient: name.into(),
        name: name.into(),
        arguments: serde_json::from_value(arguments.clone()).map_err(|_| IpcError::internal())?,
    };
    if name != "contract.request_workflow" {
        return prepare_chat_tool(&[], &call);
    }
    // Exercise valid semantics without manufacturing a trusted worker for the
    // unit-test executable. This fixture reaches scheduling only, never dispatch.
    let request: crate::evm_approval::contracts::ContractWorkflowRequest =
        serde_json::from_value(arguments).map_err(|_| IpcError::internal())?;
    request.validate()?;
    Ok(PreparedChatTool {
        request: ToolRequest {
            call_id: ToolCallId(call.call_id.clone()),
            name: ToolName::ContractRequestWorkflow,
            arguments: serde_json::from_value(serde_json::json!(call.arguments))
                .map_err(|_| IpcError::internal())?,
        },
        workspace: None,
        activity: crate::chat_runtime::tool_activity(&call.call_id, name),
        call,
    })
}

#[test]
fn valid_contract_calls_fit_the_batch_budget_and_remain_serial() -> Result<(), IpcError> {
    for (name, estimate) in [
        ("contract.inspect", 4 * 1024),
        ("contract.read", 64 * 1024),
        ("contract.request_workflow", 64 * 1024),
    ] {
        let calls = [prepared_contract(name)?];
        let plan = plan_batch(&calls);
        assert!(!plan.concurrent_reads, "{name}");
        assert_eq!(estimated_output_bytes(&calls[0].call), estimate);
        assert_eq!(
            admit_execution_order(&plan.execution_order, &calls, 8, 2 * 1024 * 1024),
            [0],
            "{name} must fit the default batch"
        );
        assert_eq!(
            admit_execution_order(&plan.execution_order, &calls, 1, estimate),
            [0]
        );
        assert!(admit_execution_order(&plan.execution_order, &calls, 1, estimate - 1).is_empty());
    }
    Ok(())
}

#[test]
fn contract_result_estimates_never_erase_executed_facts() -> Result<(), IpcError> {
    for name in ["contract.read", "contract.request_workflow"] {
        let prepared = prepared_contract(name)?;
        let mut exchange = failed_exchange(
            &prepared,
            IpcError::internal(),
            TaskActivityStatus::Completed,
            "Executed contract operation",
        );
        // Exceed the estimate deliberately: it is a scheduling reservation, not
        // authority to erase a read's evidence or a workflow's partial effects.
        let error = IpcError::invalid_request("x".repeat(64 * 1024 + 1));
        exchange.result.outcome = ToolOutcome::Failed {
            error: error.clone(),
        };
        let mut references = PathReferenceLedger::default();
        let mut bytes = 0;
        let retained = retain_scheduled_result(
            &prepared,
            Some(Ok(exchange)),
            &mut references,
            &mut bytes,
            64 * 1024,
            false,
        )?;
        assert!(
            matches!(retained.result.outcome, ToolOutcome::Failed { error: actual } if actual == error)
        );
        assert!(bytes > 64 * 1024);
    }
    Ok(())
}
