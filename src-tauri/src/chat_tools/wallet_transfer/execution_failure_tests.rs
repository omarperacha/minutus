use super::*;

#[test]
fn signing_failure_does_not_claim_preparation_failed_before_approval()
-> Result<(), Box<dyn std::error::Error>> {
    let error = IpcError::invalid_request("exact approval belongs to a different signing account");
    for (phase, state, may_have_review) in [
        (FailurePhase::Preparation, "preparationFailed", false),
        (FailurePhase::Review, "reviewFailed", true),
    ] {
        let result = failed_outcome(&error, phase).map_err(|error| error.message)?;
        let ToolOutcome::Completed { content } = result else {
            return Err("expected result".into());
        };
        let ToolResultPart::Json { value } = &content[0] else {
            return Err("expected JSON".into());
        };
        let encoded = serde_json::to_value(value)?;
        assert_eq!(encoded["state"], state);
        assert_eq!(encoded["approvalMayHaveBeenRequested"], may_have_review);
        assert_eq!(encoded["retryAutomatically"], false);
        let text = encoded.to_string();
        assert!(text.contains(state));
        assert!(text.contains(if may_have_review {
            "reviewOrSigning"
        } else {
            "preparation"
        }));
        assert!(text.contains("retryAutomatically"));
        assert!(text.contains("different signing account"));
    }
    Ok(())
}
