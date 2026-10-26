use super::*;
use crate::OutputExpectation;

fn json_request(expression: &str) -> Result<ComputationRequest, crate::ContractError> {
    ComputationRequest::new(
        "finite-output",
        format!("export default function compute(zeta) {{ return {expression}; }}"),
        Vec::new(),
        OutputExpectation::new(OutputKind::Json, 1024, 32)?,
    )
}

#[test]
fn non_finite_numbers_cannot_silently_become_json_null() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "NaN",
        "Infinity",
        "-Infinity",
        "zeta.missingLeft - zeta.missingRight",
        "({value: [1, NaN]})",
        "new Number(Infinity)",
        "({toJSON() { return NaN; }})",
        "(() => { Number.isFinite = () => true; Number.prototype.valueOf = () => 0; return new Number(NaN); })()",
    ] {
        let response = execute(&json_request(expression)?);
        assert_eq!(
            response.status(),
            RunnerStatus::RuntimeFailed,
            "{expression}"
        );
        assert!(response.output().is_none(), "{expression}");
    }
    Ok(())
}

#[test]
fn legitimate_null_zero_and_finite_numbers_remain_distinct()
-> Result<(), Box<dyn std::error::Error>> {
    let response = execute(&json_request("({empty: null, zero: 0, value: 2.5})")?);
    assert_eq!(response.status(), RunnerStatus::Completed);
    assert_eq!(
        response.output(),
        Some(&ComputationOutput::Json(serde_json::json!({
            "empty": null, "zero": 0, "value": 2.5
        })))
    );
    Ok(())
}
