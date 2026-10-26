//! Advisory checkpoint identity is separate from legacy evidence equivalence.

use super::*;

fn ready_write_policy() -> Result<(PlanPolicy, Vec<PromptToolDefinition>), IpcError> {
    let all = vec![
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool("computation.run"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;
    policy.complete_advisory_stage(&stage_completion("read-source"))?;
    Ok((policy, all))
}

#[test]
fn advisory_revision_reopens_changed_action_or_dependencies() -> Result<(), IpcError> {
    for change_action in [true, false] {
        let (mut policy, all) = ready_write_policy()?;
        let mut revision = semantic_v6_plan_call("revise");
        let steps = revision
            .arguments
            .get_mut("steps")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(IpcError::internal)?;
        if change_action {
            steps[0]["actionClass"] = serde_json::json!("compute");
            steps[0]["suggestedTools"] = serde_json::json!(["computation.run"]);
        } else {
            let mut prerequisite = steps[0].clone();
            prerequisite["id"] = serde_json::json!("new-input");
            steps[0]["dependsOn"] = serde_json::json!(["new-input"]);
            steps.insert(0, prerequisite);
        }
        policy.accept(&revision, &all, 10)?;
        assert!(
            !policy
                .forecast_steps
                .iter()
                .find(|step| step.id == "read-source")
                .ok_or_else(IpcError::internal)?
                .complete()
        );
        assert!(!policy.planned_outcome_satisfied());
    }
    Ok(())
}

#[test]
fn retained_read_checkpoint_cannot_verify_a_changed_output_target() -> Result<(), IpcError> {
    let (mut policy, all) = ready_write_policy()?;
    policy.record_results(
        &[operation("old-write", "filesystem.write_text")],
        &[Ok(completed_operation(
            "old-write",
            ToolName::FilesystemWriteText,
        ))],
    );
    assert!(policy.planned_outcome_satisfied());
    let mut revision = semantic_v6_plan_call("revise");
    revision.arguments.insert(
        "target".to_owned(),
        serde_json::json!("Create a different output"),
    );
    policy.accept(&revision, &all, 10)?;
    assert!(policy.forecast_steps[0].complete());
    assert!(!policy.forecast_steps[1].complete());
    assert!(!policy.planned_outcome_satisfied());
    assert!(policy.verified_outcome_call_ids.is_empty());
    assert!(!policy.final_response_ready());
    Ok(())
}

#[test]
fn effect_stage_completion_still_requires_exact_work_equivalence() -> Result<(), IpcError> {
    let (mut policy, all) = ready_write_policy()?;
    policy.record_results(
        &[operation("old-write", "filesystem.write_text")],
        &[Ok(completed_operation(
            "old-write",
            ToolName::FilesystemWriteText,
        ))],
    );
    let mut revision = semantic_v6_plan_call("revise");
    *revision
        .arguments
        .get_mut("steps")
        .and_then(|steps| steps.pointer_mut("/1/doneWhen"))
        .ok_or_else(IpcError::internal)? =
        serde_json::json!("The expanded deliverable is verified");
    policy.accept(&revision, &all, 10)?;
    assert!(policy.forecast_steps[0].complete());
    assert!(!policy.forecast_steps[1].complete());
    assert!(!policy.final_response_ready());
    Ok(())
}
