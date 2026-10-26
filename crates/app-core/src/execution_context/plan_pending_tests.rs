//! Planning deferral has one required control without altering direct-web policy.
use super::*;

#[test]
fn pending_plan_projects_alone_guidance_and_web_without_pending_has_no_tail() {
    let context = ToolOrchestrationContext::new(
        ToolOperationBudget::new(1, 8, 2, 10, 0, 1),
        true,
        ToolOrchestrationPhase::WebAnswer,
    );
    assert!(context.render_tail().is_empty());
    let stable = context.stable_policy();
    assert!(stable.contains("plan alone"));
    let pending = context.with_recovery_diagnostic(Some(ToolRecoveryDiagnostic::new(
        "plan_pending".into(),
        "Submit orchestration.submit_plan alone".into(),
        1,
        2,
    )));
    assert_eq!(pending.stable_policy(), stable);
    let tail = pending.render_tail();
    assert!(tail.contains("\"instruction\":\"submit_plan_only\""));
    assert!(tail.contains("\"requiredNextOperation\":\"orchestration.submit_plan\""));
    assert!(!tail.contains("emit_outcome_operation_now"));
}
