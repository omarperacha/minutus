use super::{
    HOST_ROUTED_SEMANTIC_PLAN_CONTRACT_VERSION, IpcError, OUTCOME_SEMANTIC_PLAN_CONTRACT_VERSION,
    SEMANTIC_PLAN_CONTRACT_VERSION,
};

pub(super) fn plan_repair_defects(error: &IpcError) -> Option<Vec<(String, String)>> {
    let (contract_version, payload) = [
        OUTCOME_SEMANTIC_PLAN_CONTRACT_VERSION,
        HOST_ROUTED_SEMANTIC_PLAN_CONTRACT_VERSION,
        SEMANTIC_PLAN_CONTRACT_VERSION,
    ]
    .into_iter()
    .find_map(|version| {
        error
            .message
            .strip_prefix(&format!("{version} plan defects: "))
            .map(|payload| (version, payload))
    })?;
    let payload = serde_json::from_str::<serde_json::Value>(payload).ok()?;
    let defects = payload.get("defects")?.as_array()?;
    let parsed = defects
        .iter()
        .filter_map(|defect| {
            let code = defect.get("code")?.as_str()?;
            let step_id = defect
                .get("stepId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("plan");
            let path = defect.get("path")?.as_str()?;
            let received = defect.get("received").unwrap_or(&serde_json::Value::Null);
            Some((
                format!("{contract_version}:{code}:{step_id}:{path}"),
                repair_identity(code, received),
            ))
        })
        .collect::<Vec<_>>();
    (!parsed.is_empty()).then_some(parsed)
}

// Wrong-type payload edits do not repair a structural defect. Display-oriented
// string truncation (type + length) must not turn paraphrases into progress.
fn repair_identity(code: &str, received: &serde_json::Value) -> String {
    if code == "invalid_array" && !received.is_array() {
        return "invalid_array_type".to_owned();
    }
    serde_json::to_string(received).unwrap_or_default()
}
