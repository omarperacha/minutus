//! Captured malformed shapes are rejection fixtures, never suggested requests.
use super::*;
use crate::{
    chat_tools::{
        contracts::declared_workflow_schema_for_tests,
        validation::{exact_argument_repair_error, validated_chat_tool_request},
    },
    evm_approval::contracts::DeclaredWorkflowRequest,
};
use ipc_types::IpcErrorCode;
use model_runtime::ParsedToolCall;

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn tool() -> PromptToolDefinition {
    declared_workflow_schema_for_tests()
}
fn call(value: Value) -> Result<ParsedToolCall, serde_json::Error> {
    Ok(ParsedToolCall {
        call_id: "workflow-schema-repair".into(),
        recipient: "contract.request_workflow".into(),
        name: "contract.request_workflow".into(),
        arguments: serde_json::from_value(value)?,
    })
}
fn diagnostics(message: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let encoded = message
        .split_once("Diagnostics: ")
        .ok_or("diagnostics")?
        .1
        .split_once(". Additional locations not inspected:")
        .ok_or("diagnostic boundary")?
        .0;
    Ok(serde_json::from_str(encoded)?)
}

#[test]
fn generic_advertised_example_passes_production_declaration_validation() -> TestResult {
    let value: Value = serde_json::from_str(WORKFLOW_V2_EXAMPLE)?;
    let request: DeclaredWorkflowRequest = serde_json::from_value(value.clone())?;
    request.validate().map_err(|error| error.message)?;
    let call = call(value)?;
    // Packaged/release availability remains independent of request validity.
    if let Err(error) = validated_chat_tool_request(&call, &[]) {
        assert_eq!(
            error.code,
            IpcErrorCode::PermissionDenied,
            "{}",
            error.message
        );
    }
    let definition = tool();
    assert!(definition.description.contains(WORKFLOW_V2_EXAMPLE));
    assert!(
        definition
            .description
            .contains("actually inspected user scope")
    );
    assert!(
        repair_error(
            &definition,
            &IpcError::invalid_request("semantic rejection"),
            &serde_json::from_str(WORKFLOW_V2_EXAMPLE)?
        )
        .is_none()
    );
    Ok(())
}

#[test]
fn actual_flat_version1_and_version2_shapes_report_simultaneous_paths_without_rewriting()
-> TestResult {
    let values: Vec<Value> = serde_json::from_str(include_str!("malformed-workflows.json"))?;
    let required = [
        vec![
            "$.contractVersion",
            "$.contracts",
            "$.steps",
            "$.goals",
            "$.arguments",
            "$.block",
            "$.target",
            "$.maxFeePerGasWei",
        ],
        vec![
            "$.contractVersion",
            "$.contracts[0].id",
            "$.contracts[0].function",
            "$.steps[0].id",
            "$.steps[0].contract",
            "$.steps[0].dependsOn",
            "$.steps[0].prerequisites",
            "$.steps[0].completion",
            "$.steps[0].arguments[0]",
            "$.steps[0].valueWei",
            "$.goals[0]",
        ],
        vec![
            "$.contracts[0].id",
            "$.contracts[0].maxFeePerGasWei",
            "$.steps[0].id",
            "$.steps[0].contract",
            "$.steps[0].dependsOn",
            "$.steps[0].prerequisites",
            "$.steps[0].completion",
            "$.steps[0].arguments[0]",
            "$.steps[0].valueWei",
            "$.goals[0]",
        ],
    ];
    for (value, paths) in values.into_iter().zip(required) {
        assert!(serde_json::from_value::<DeclaredWorkflowRequest>(value.clone()).is_err());
        let call = call(value)?;
        let original = call.arguments.clone();
        let error = IpcError::invalid_request("Original native declaration rejection");
        let repair = exact_argument_repair_error(&call, &[tool()], &error);
        let defects = diagnostics(&repair.message)?;
        for path in paths {
            assert!(
                defects.iter().any(|defect| defect["path"] == path),
                "{path}: {}",
                repair.message
            );
        }
        assert!(defects.len() <= MAX_DEFECTS);
        assert!(repair.message.len() < MAX_MESSAGE_BYTES);
        assert!(!repair.message.contains("\"properties\""));
        assert_eq!(repair.code, error.code);
        assert_eq!(repair.retryable, error.retryable);
        assert_eq!(call.arguments, original);
    }
    Ok(())
}

#[test]
fn nested_native_arrays_closed_fields_and_enums_are_reported_together() -> TestResult {
    let mut value: Value = serde_json::from_str(WORKFLOW_V2_EXAMPLE)?;
    value["contractVersion"] = json!(1);
    value["steps"][0]["arguments"] = json!("[\"7\"]");
    value["steps"][0]["dependsOn"] = json!("[]");
    value["steps"][0]["approved"] = json!("private payload must not echo");
    value["goals"] = json!([{"contract":"example","read":{"kind":"abiRead","path":"number","outputs":[]},"comparison":"notEqual","expected":7}]);
    let repair =
        repair_error(&tool(), &IpcError::invalid_request("rejected"), &value).ok_or("repair")?;
    let defects = diagnostics(&repair.message)?;
    for path in [
        "$.contractVersion",
        "$.steps[0].arguments",
        "$.steps[0].dependsOn",
        "$.steps[0].approved",
        "$.goals[0].read.kind",
        "$.goals[0].read.path",
        "$.goals[0].read.outputs",
        "$.goals[0].comparison",
        "$.goals[0].expected",
    ] {
        assert!(
            defects.iter().any(|defect| defect["path"] == path),
            "{path}"
        );
    }
    assert!(repair.message.contains("the constant 2"));
    assert!(repair.message.contains("native JSON type"));
    assert!(!repair.message.contains("private payload"));
    Ok(())
}

#[test]
fn many_defects_are_deterministic_bounded_and_explicitly_incomplete() -> TestResult {
    let value = json!({"contractVersion":1,"contracts":vec![json!({});16],"steps":vec![json!({});16],"goals":[]});
    let error = IpcError::invalid_request("not a source of additional unbounded diagnostics");
    let first = repair_error(&tool(), &error, &value).ok_or("repair")?;
    let second = repair_error(&tool(), &error, &value).ok_or("repair")?;
    assert_eq!(first.message, second.message);
    assert!(first.message.len() <= MAX_MESSAGE_BYTES);
    assert!(diagnostics(&first.message)?.len() <= MAX_DEFECTS);
    assert!(
        first
            .message
            .contains("Additional locations not inspected: true")
    );
    assert!(!first.message.contains("unbounded diagnostics"));
    Ok(())
}

#[test]
fn diagnostic_visit_and_path_bounds_never_claim_complete_or_exact_coverage() -> TestResult {
    let mut value: Value = serde_json::from_str(WORKFLOW_V2_EXAMPLE)?;
    let mut step = value["steps"][0].clone();
    step["arguments"] = json!(vec![vec!["7"; 32]; 16]);
    value["steps"] = json!(vec![step; 16]);
    let error = IpcError::invalid_request("original rejection");
    let repair =
        repair_error(&tool(), &error, &value).ok_or("bounded traversal must not fall through")?;
    assert!(repair.message.len() <= MAX_MESSAGE_BYTES);
    assert!(
        repair
            .message
            .contains("Additional locations not inspected: true")
    );
    let mut value: Value = serde_json::from_str(WORKFLOW_V2_EXAMPLE)?;
    let object = value.as_object_mut().ok_or("object")?;
    object.insert("unrecognized".repeat(80), json!("do not echo this value"));
    let repair = repair_error(&tool(), &error, &value).ok_or("clipped path")?;
    assert!(
        diagnostics(&repair.message)?
            .iter()
            .any(|item| item["pathTruncated"] == true)
    );
    assert!(!repair.message.contains("do not echo this value"));
    Ok(())
}
