use super::{
    BTreeMap, BTreeSet, IpcError, PlanActionClass, PlanContractVersion, PlanEvidenceClass,
    PlanEvidenceObligation, PlanEvidencePostcondition, PlanEvidenceRoleKind, SemanticPlanContract,
    SemanticPlanStep, bounded_string, bounded_string_array, contract_error,
    order_satisfiable_dependencies, valid_obligation_id,
};

#[derive(Clone)]
pub(super) struct V4StepDraft {
    pub(super) id: String,
    pub(super) objective: String,
    pub(super) depends_on: Vec<String>,
    pub(super) obligations: Vec<PlanEvidenceObligation>,
    pub(super) done_when: String,
    pub(super) action_class: PlanActionClass,
}

#[expect(
    clippy::too_many_lines,
    reason = "v4 aggregates the complete closed plan contract in one validation pass"
)]
pub(super) fn parse_v4_contract(
    arguments: &BTreeMap<String, serde_json::Value>,
    maximum_steps: usize,
    contract_version: PlanContractVersion,
) -> Result<SemanticPlanContract, IpcError> {
    let target = arguments
        .get("target")
        .and_then(|value| bounded_string(value, 1, 512))
        .unwrap_or_default();
    let constraints = bounded_string_array(arguments, "constraints", 0, 8).unwrap_or_default();
    let success_criteria =
        bounded_string_array(arguments, "successCriteria", 1, 8).unwrap_or_default();
    let assumptions = bounded_string_array(arguments, "assumptions", 0, 8).unwrap_or_default();
    let values = arguments
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .filter(|steps| !steps.is_empty() && steps.len() <= maximum_steps)
        .ok_or_else(|| contract_error("The semantic plan must contain one to sixteen steps"))?;
    let mut defects = Vec::new();
    let mut step_ids = BTreeSet::new();
    let mut obligation_ids = BTreeSet::new();
    let mut drafts = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let path = format!("/steps/{index}");
        let Some(object) = value.as_object() else {
            defects.push(v4_defect(
                "invalid_step",
                None,
                &path,
                value,
                &serde_json::json!({"type":"object"}),
            ));
            continue;
        };
        let id = object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let step_id = (!id.is_empty()).then_some(id);
        if !valid_obligation_id(id) || !step_ids.insert(id.to_owned()) {
            defects.push(v4_defect(
                "invalid_step_id",
                step_id,
                &format!("{path}/id"),
                object.get("id").unwrap_or(&serde_json::Value::Null),
                &serde_json::json!({"pattern":"^[a-z][a-z0-9-]{0,31}$","unique":true}),
            ));
            continue;
        }
        let action = object
            .get("actionClass")
            .and_then(serde_json::Value::as_str)
            .and_then(|action| match action {
                "read" => Some(PlanActionClass::Read),
                "compute" => Some(PlanActionClass::Compute),
                "workspace_write" => Some(PlanActionClass::WorkspaceWrite),
                "external_write" => Some(PlanActionClass::ExternalWrite),
                _ => None,
            });
        let Some(action_class) = action else {
            defects.push(v4_defect(
                "invalid_action_class",
                step_id,
                &format!("{path}/actionClass"),
                object
                    .get("actionClass")
                    .unwrap_or(&serde_json::Value::Null),
                &serde_json::json!(["read", "compute", "workspace_write", "external_write"]),
            ));
            continue;
        };
        let objective = object
            .get("objective")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let done_when = object
            .get("doneWhen")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let depends_on = v4_string_array(object, "dependsOn", id, &path, &mut defects);
        let obligation_values = object
            .get("obligations")
            .and_then(serde_json::Value::as_array);
        let mut obligations = Vec::new();
        let mut outcome_count = 0_usize;
        if let Some(obligation_values) = obligation_values {
            for (obligation_index, obligation) in obligation_values.iter().enumerate() {
                if let Some((parsed, is_outcome)) = parse_v4_obligation(
                    obligation,
                    id,
                    action_class,
                    &format!("{path}/obligations/{obligation_index}"),
                    &mut defects,
                ) {
                    if obligation_ids.insert(parsed.id.clone()) {
                        outcome_count += usize::from(is_outcome);
                        obligations.push(parsed);
                    } else {
                        defects.push(v4_defect(
                            "duplicate_obligation_id",
                            Some(id),
                            &format!("{path}/obligations/{obligation_index}/id"),
                            &serde_json::json!(parsed.id),
                            &serde_json::json!({"uniqueAcrossPlan":true}),
                        ));
                    }
                }
            }
        } else {
            defects.push(v4_defect(
                "missing_obligations",
                Some(id),
                &format!("{path}/obligations"),
                &serde_json::Value::Null,
                &permitted_obligations(action_class),
            ));
        }
        let valid_cardinality = match action_class {
            PlanActionClass::Read => !obligations.is_empty() && outcome_count == 0,
            _ => !obligations.is_empty() && outcome_count == 1,
        };
        if !valid_cardinality {
            defects.push(v4_defect(
                "invalid_step_obligations",
                Some(id),
                &format!("{path}/obligations"),
                object
                    .get("obligations")
                    .unwrap_or(&serde_json::Value::Null),
                &permitted_obligations(action_class),
            ));
        }
        drafts.push(V4StepDraft {
            id: id.to_owned(),
            objective,
            depends_on,
            obligations,
            done_when,
            action_class,
        });
    }
    if !defects.is_empty() {
        return Err(v4_contract_error(&defects, contract_version));
    }
    let mut steps = drafts
        .iter()
        .map(|draft| SemanticPlanStep {
            id: draft.id.clone(),
            objective: draft.objective.clone(),
            depends_on: draft.depends_on.clone(),
            required_evidence: Vec::new(),
            obligations: draft.obligations.clone(),
            done_when: draft.done_when.clone(),
            action_class: draft.action_class,
            preferred_tools: Vec::new(),
        })
        .collect::<Vec<_>>();
    steps = order_satisfiable_dependencies(steps)?;
    Ok(SemanticPlanContract {
        target,
        constraints,
        success_criteria,
        assumptions,
        steps,
    })
}

pub(super) fn v4_string_array(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    step_id: &str,
    path: &str,
    defects: &mut Vec<serde_json::Value>,
) -> Vec<String> {
    let Some(values) = object.get(field).and_then(serde_json::Value::as_array) else {
        defects.push(v4_defect(
            "invalid_string_array",
            Some(step_id),
            &format!("{path}/{field}"),
            object.get(field).unwrap_or(&serde_json::Value::Null),
            &serde_json::json!({"type":"array","uniqueItems":true}),
        ));
        return Vec::new();
    };
    values
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect()
}

pub(super) fn parse_v4_obligation(
    value: &serde_json::Value,
    step_id: &str,
    action: PlanActionClass,
    path: &str,
    defects: &mut Vec<serde_json::Value>,
) -> Option<(PlanEvidenceObligation, bool)> {
    let Some(object) = value.as_object() else {
        defects.push(v4_defect(
            "invalid_obligation",
            Some(step_id),
            path,
            value,
            &permitted_obligations(action),
        ));
        return None;
    };
    let Some(id) = object.get("id").and_then(serde_json::Value::as_str) else {
        defects.push(v4_defect(
            "invalid_obligation_id",
            Some(step_id),
            &format!("{path}/id"),
            object.get("id").unwrap_or(&serde_json::Value::Null),
            &serde_json::json!({"pattern":"^[a-z][a-z0-9-]{0,31}$"}),
        ));
        return None;
    };
    if !valid_obligation_id(id) {
        defects.push(v4_defect(
            "invalid_obligation_id",
            Some(step_id),
            &format!("{path}/id"),
            &serde_json::json!(id),
            &serde_json::json!({"pattern":"^[a-z][a-z0-9-]{0,31}$"}),
        ));
        return None;
    }
    let kind = object.get("kind").and_then(serde_json::Value::as_str);
    let evidence =
        kind.and_then(parse_v4_requirement)
            .and_then(|(evidence_class, postcondition)| {
                let role = object
                    .get("roleKind")
                    .and_then(serde_json::Value::as_str)
                    .and_then(parse_v4_role);
                (object.len() == 3).then_some(())?;
                role.map(|role_kind| PlanEvidenceObligation {
                    id: id.to_owned(),
                    role_kind,
                    evidence_class,
                    postcondition,
                })
            });
    if let Some(evidence) = evidence {
        return Some((evidence, false));
    }
    let outcome = kind
        .and_then(|kind| parse_v4_outcome(action, kind))
        .filter(|_| object.len() == 2)
        .map(|(evidence_class, postcondition)| PlanEvidenceObligation {
            id: id.to_owned(),
            role_kind: PlanEvidenceRoleKind::Target,
            evidence_class,
            postcondition,
        });
    if let Some(outcome) = outcome {
        return Some((outcome, true));
    }
    defects.push(v4_defect(
        "invalid_obligation_kind",
        Some(step_id),
        path,
        value,
        &permitted_obligations(action),
    ));
    None
}

pub(super) fn parse_v4_role(value: &str) -> Option<PlanEvidenceRoleKind> {
    match value {
        "source" => Some(PlanEvidenceRoleKind::Source),
        "convention" => Some(PlanEvidenceRoleKind::Convention),
        "target" => Some(PlanEvidenceRoleKind::Target),
        "input" => Some(PlanEvidenceRoleKind::Input),
        "reference" => Some(PlanEvidenceRoleKind::Reference),
        _ => None,
    }
}

pub(super) fn parse_v4_requirement(
    value: &str,
) -> Option<(PlanEvidenceClass, PlanEvidencePostcondition)> {
    use PlanEvidenceClass::{FormatStructure, IdentityLocation, SemanticContent};
    use PlanEvidencePostcondition::{
        ContentLoaded, FormatLoaded, IdentityLocated, ImageContentLoaded, ImageFormatLoaded,
        TextContentLoaded, TextFormatLoaded,
    };
    match value {
        "identity_located" => Some((IdentityLocation, IdentityLocated)),
        "format_loaded" => Some((FormatStructure, FormatLoaded)),
        "text_format_loaded" => Some((FormatStructure, TextFormatLoaded)),
        "image_format_loaded" => Some((FormatStructure, ImageFormatLoaded)),
        "content_loaded" => Some((SemanticContent, ContentLoaded)),
        "text_content_loaded" => Some((SemanticContent, TextContentLoaded)),
        "image_content_loaded" => Some((SemanticContent, ImageContentLoaded)),
        _ => None,
    }
}

pub(super) fn parse_v4_outcome(
    action: PlanActionClass,
    value: &str,
) -> Option<(PlanEvidenceClass, PlanEvidencePostcondition)> {
    use PlanEvidenceClass::{IdentityLocation, SemanticContent};
    use PlanEvidencePostcondition::{
        ComputationCompleted, DirectoryCreateVerified, ExternalEffectVerified, FileWriteVerified,
    };
    match (action, value) {
        (PlanActionClass::Compute, "computation_completed") => {
            Some((SemanticContent, ComputationCompleted))
        }
        (PlanActionClass::WorkspaceWrite, "file_write_verified") => {
            Some((IdentityLocation, FileWriteVerified))
        }
        (PlanActionClass::WorkspaceWrite, "directory_create_verified") => {
            Some((IdentityLocation, DirectoryCreateVerified))
        }
        (PlanActionClass::ExternalWrite, "external_effect_verified") => {
            Some((IdentityLocation, ExternalEffectVerified))
        }
        _ => None,
    }
}

pub(super) fn permitted_obligations(action: PlanActionClass) -> serde_json::Value {
    let mut replacements = [
        "identity_located",
        "format_loaded",
        "text_format_loaded",
        "image_format_loaded",
        "content_loaded",
        "text_content_loaded",
        "image_content_loaded",
    ]
    .into_iter()
    .map(|kind| serde_json::json!({"id":"task-local-id","kind":kind,"roleKind":"source"}))
    .collect::<Vec<_>>();
    let outcome_count = usize::from(action != PlanActionClass::Read);
    match action {
        PlanActionClass::Read => {}
        PlanActionClass::Compute => replacements
            .push(serde_json::json!({"id":"task-local-id","kind":"computation_completed"})),
        PlanActionClass::WorkspaceWrite => replacements.extend([
            serde_json::json!({"id":"task-local-id","kind":"file_write_verified"}),
            serde_json::json!({"id":"task-local-id","kind":"directory_create_verified"}),
        ]),
        PlanActionClass::ExternalWrite => replacements
            .push(serde_json::json!({"id":"task-local-id","kind":"external_effect_verified"})),
    }
    serde_json::json!({"replaceWithOneOf":replacements,"requiredOutcomeCount":outcome_count})
}

pub(super) fn v4_defect(
    code: &str,
    step_id: Option<&str>,
    path: &str,
    received: &serde_json::Value,
    permitted: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({"code":code,"stepId":step_id,"path":path,"received":received,"permitted":permitted})
}

pub(super) fn v4_contract_error(
    defects: &[serde_json::Value],
    contract_version: PlanContractVersion,
) -> IpcError {
    contract_error(&format!(
        "{} plan defects: {}",
        contract_version.as_str(),
        serde_json::json!({"defects":defects,"repair":"Preserve unrelated plan content and replace only the indicated objects."})
    ))
}
