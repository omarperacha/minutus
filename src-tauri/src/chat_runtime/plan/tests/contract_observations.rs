//! Scripted policy trajectories, not live-model or native-UI evidence.
use super::*;

#[test]
fn unsigned_contract_reads_require_semantic_plans_and_discoverable_read_schemas()
-> Result<(), IpcError> {
    let all = crate::chat_tools::chat_tool_definitions(&[])?;
    let mut policy = PlanPolicy::default();
    let initial = policy.tools(&all);
    assert!(initial.iter().any(|tool| tool.name == "contract.inspect"));
    assert!(!initial.iter().any(|tool| tool.name == "contract.read"));
    let read = operation("read", "contract.read");
    assert!(policy.initial_plan_required(std::slice::from_ref(&read)));
    assert!(policy.validate_calls(std::slice::from_ref(&read)).is_err());
    let plan=ParsedToolCall{call_id:"plan-contract".into(),recipient:PLAN_TOOL.into(),name:PLAN_TOOL.into(),arguments:serde_json::from_value(serde_json::json!({
        "contractVersion":crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION,"mode":"initial","outcome":"other",
        "target":"Observe an allowance without changing it","constraints":["No signing or allowance mutation"],"assumptions":[],"successCriteria":["Report the allowance at the observed block"],
        "steps":[{"id":"read","actionClass":"read","dependsOn":[],"objective":"Read the exact public allowance","doneWhen":"The allowance value is observed at a canonical block","suggestedTools":["contract.read"]}]
    })).unwrap_or_default()};
    policy.accept(&plan, &all, 8)?;
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "contract.read")
    );
    policy.validate_calls(&[read])?;
    assert_eq!(
        crate::chat_tools::tool_action_class("contract.read"),
        Some(crate::chat_tools::ToolActionClass::Read)
    );
    assert!(!all.iter().any(|tool| tool.name == "contract.request_write"));
    Ok(())
}
