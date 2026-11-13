use super::*;
use crate::chat_tools::ChatToolExchange;
use ipc_types::{TaskActivity, TaskActivityKind, TaskActivityStatus, ToolName, ToolOutcome};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn baseline() -> Result<Vec<ChatToolExchange>, Box<dyn std::error::Error>> {
    // Captured public host exchanges from request 01a1d46d-1125-73f0-8fb0-cdfa1efa2d7c.
    // Historical playback only: no receipt/proof/cause is minted by this fixture.
    let values: Vec<Value> = serde_json::from_str(include_str!("public_postsubmit_fixture.json"))?;
    values
        .into_iter()
        .map(|value| {
            Ok(ChatToolExchange {
                request: serde_json::from_value(value["request"].clone())?,
                result: serde_json::from_value(value["result"].clone())?,
                activity: TaskActivity {
                    id: "fixture".into(),
                    kind: TaskActivityKind::TransactionStatus,
                    title: "Historical public fixture".into(),
                    status: TaskActivityStatus::Partial,
                    detail: None,
                },
                workspace_grant_id: None,
                undo_changes: vec![],
                inspected_asset_id: None,
                computation_reuse_digest: None,
            })
        })
        .collect()
}

#[test]
fn actual_submitted_failure_and_later_read_42_fit_without_upgrading_workflow() -> TestResult {
    let exchanges = baseline()?;
    let report = from_exchange(&exchanges[0]).ok_or("missing native report")?;
    let separate = report
        .observation(&exchanges[1], true)
        .ok_or("missing separate read")?;
    let output = format!("{}{separate}", report.render());
    assert_eq!(output, include_str!("public_postsubmit_report.md"));
    // An actual vocabulary-only run measured this exact historical report.
    // Runtime admission independently recounts using the selected live binding.
    let measured: Value =
        serde_json::from_str(include_str!("public_postsubmit_token_evidence.json"))?;
    assert_eq!(
        format!("{:x}", Sha256::digest(output.as_bytes())),
        measured["reportSha256"]
    );
    assert!(
        measured["exactTokens"].as_u64().ok_or("measured count")?
            < measured["observedFinalOutputCap"]
                .as_u64()
                .ok_or("observed cap")?
    );
    assert!(output.len() < MAX_REPORT_BYTES);
    assert!(output.contains("Submission was acknowledged"));
    assert!(output.contains("Receipt observation unavailable"));
    assert!(output.contains("0xa336f54ab97a99dc8a5aba43f6d43b3011121f1928c3a3f470808ec847283989"));
    assert!(output.contains("postSubmission\":\"notRecorded"));
    assert!(output.contains("Later separate unsigned observation"));
    assert!(output.contains("\"values\":[\"42\"]"));
    assert!(output.contains("12125396"));
    assert!(!output.contains("durableState"));
    assert!(!output.contains("authorizationEvents"));
    assert!(!output.contains("Contract workflow: verified"));
    eprintln!(
        "Historical public workflow report: {} bytes (exact token admission is separately required at runtime)",
        output.len()
    );
    Ok(())
}

#[test]
fn read_lookalike_foreign_scope_and_unaccounted_material_fail_closed() -> TestResult {
    let mut exchanges = baseline()?;
    let report = from_exchange(&exchanges[0]).ok_or("native report")?;
    exchanges[0].request.name = ToolName::ContractRead;
    assert!(from_exchange(&exchanges[0]).is_none());
    exchanges[1].request.arguments.insert(
        "network".into(),
        ipc_types::JsonValue::String("baseSepolia".into()),
    );
    assert!(report.observation(&exchanges[1], true).is_none());
    let mut exchanges = baseline()?;
    let ToolOutcome::Failed { error } = &mut exchanges[0].result.outcome else {
        return Err("failure fixture".into());
    };
    let mut value: Value = serde_json::from_str(&error.message)?;
    value["futureMaterialFacts"] = json!("must not silently omit");
    error.message = value.to_string();
    assert!(from_exchange(&exchanges[0]).is_none());
    Ok(())
}

#[test]
fn only_expected_nonmaterial_boilerplate_can_be_omitted() -> TestResult {
    for (field, replacement) in [
        ("automaticCleanup", json!(true)),
        ("currentWalletState", json!("locked")),
    ] {
        let mut exchanges = baseline()?;
        let ToolOutcome::Failed { error } = &mut exchanges[0].result.outcome else {
            return Err("failure fixture".into());
        };
        let mut value: Value = serde_json::from_str(&error.message)?;
        value[field] = replacement;
        error.message = value.to_string();
        assert!(from_exchange(&exchanges[0]).is_none());
    }
    Ok(())
}

#[test]
fn model_presentation_summary_is_never_rendered_as_a_host_fact() -> TestResult {
    let mut exchanges = baseline()?;
    let mut arguments = serde_json::to_value(&exchanges[0].request.arguments)?;
    arguments["steps"][0]["summary"] = json!("MODEL CLAIM: verified safe; business goal completed");
    exchanges[0].request.arguments = serde_json::from_value(arguments)?;
    let report = from_exchange(&exchanges[0]).ok_or("host report")?;
    assert!(!report.render().contains("MODEL CLAIM"));
    assert!(!report.render().contains("summary\":"));
    assert!(report.render().contains("store(uint256)"));
    assert!(report.render().contains("\"arguments\":[\"42\"]"));
    Ok(())
}

#[test]
fn oversized_material_is_not_truncated_and_untrusted_format_controls_are_visible() -> TestResult {
    let mut exchanges = baseline()?;
    let ToolOutcome::Failed { error } = &mut exchanges[0].result.outcome else {
        return Err("failure fixture".into());
    };
    let mut value: Value = serde_json::from_str(&error.message)?;
    value["message"] = json!("x".repeat(MAX_REPORT_BYTES));
    error.message = value.to_string();
    assert!(from_exchange(&exchanges[0]).is_none());
    let original = json!({"untrusted":"```\u{202e}spoof\u{200b}"});
    let escaped = display::json(&original);
    assert!(!escaped.contains('`'));
    assert!(!escaped.contains('\u{202e}'));
    assert!(escaped.contains("\\u202e"));
    assert_eq!(serde_json::from_str::<Value>(&escaped)?, original);
    Ok(())
}

#[test]
fn typed_post_submission_failure_and_prior_completion_facts_are_preserved() -> TestResult {
    let mut exchanges = baseline()?;
    let ToolOutcome::Failed { error } = &mut exchanges[0].result.outcome else {
        return Err("failure fixture".into());
    };
    let mut value: Value = serde_json::from_str(&error.message)?;
    value["lifecycle"]["observations"]["steps"][0]["postSubmission"] = json!({
        "receipt":"unavailable","monitorAttempts":2,"lastMonitorFailure":{"stage":"receiptRpc","cause":{"kind":"transport"},"elapsedMs":3456},
        "inclusionProof":"notStarted","completionProof":"notStarted","stopReason":"monitorRejected"});
    value["partialFacts"]["historicalCompletions"] = json!([{"transactionHash":format!("0x{}","1".repeat(64)),"stateBlockNumber":"12125380","observations":[{"value":"7"}]}]);
    error.message = value.to_string();
    let report = from_exchange(&exchanges[0]).ok_or("native report")?;
    assert!(report.render().contains("monitorRejected"));
    assert!(report.render().contains("historicalCompletions"));
    assert!(report.render().contains("12125380"));
    assert!(report.render().contains("\"value\":\"7\""));
    Ok(())
}
