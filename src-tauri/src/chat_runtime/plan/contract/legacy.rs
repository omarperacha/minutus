use super::{
    BTreeSet, IpcError, PlanActionClass, PlanContractVersion, PlanEvidenceClass,
    PlanEvidenceObligation, PlanEvidencePostcondition, PlanEvidenceRoleKind, SemanticPlanStep,
    bounded_object_string, bounded_object_string_array, bounded_string, contract_error,
};

pub(super) fn parse_step(
    index: usize,
    step: &serde_json::Value,
    ids: &mut BTreeSet<String>,
    obligation_ids: &mut BTreeSet<String>,
    available_tools: &BTreeSet<&str>,
    contract_version: PlanContractVersion,
) -> Result<SemanticPlanStep, IpcError> {
    let object = step
        .as_object()
        .ok_or_else(|| contract_error(&format!("Plan step at steps[{index}] must be an object")))?;
    let allowed = if contract_version == PlanContractVersion::SemanticExecutionV3 {
        BTreeSet::from([
            "actionClass",
            "dependsOn",
            "doneWhen",
            "id",
            "objective",
            "obligations",
        ])
    } else {
        BTreeSet::from([
            "actionClass",
            "dependsOn",
            "doneWhen",
            "id",
            "objective",
            "preferredTools",
            "requiredEvidence",
        ])
    };
    if !object.keys().all(|key| allowed.contains(key.as_str())) {
        return Err(contract_error(&format!(
            "Plan step at steps[{index}] contains an unknown field"
        )));
    }
    let id = bounded_object_string(object, "id", 1, 32, index)?;
    if !ids.insert(id.clone()) {
        return Err(contract_error(&format!(
            "Plan step identifier `{id}` is duplicated"
        )));
    }
    let objective = bounded_object_string(object, "objective", 1, 256, index)?;
    let done_when = bounded_object_string(object, "doneWhen", 1, 256, index)?;
    let action_class = PlanActionClass::parse(
        object.get("actionClass").ok_or_else(|| {
            contract_error(&format!(
                "Plan step `{id}` is missing required field actionClass"
            ))
        })?,
        &id,
        contract_version,
    )?;
    let depends_on = bounded_object_string_array(object, "dependsOn", 0, 15, &id)?;
    let required_evidence = if contract_version == PlanContractVersion::SemanticExecutionV3 {
        Vec::new()
    } else {
        object
            .get("requiredEvidence")
            .and_then(serde_json::Value::as_array)
            .filter(|items| items.len() <= 3)
            .ok_or_else(|| {
                contract_error(&format!(
                    "Plan step `{id}` must encode requiredEvidence as a bounded array"
                ))
            })?
            .iter()
            .map(|value| PlanEvidenceClass::parse(value, &id))
            .collect::<Result<Vec<_>, _>>()?
    };
    let obligations = if contract_version == PlanContractVersion::SemanticExecutionV3 {
        parse_obligations(object, &id, action_class, obligation_ids)?
    } else {
        Vec::new()
    };
    let preferred_tools = if contract_version == PlanContractVersion::SemanticExecutionV3 {
        Vec::new()
    } else {
        parse_preferred_tools(object, &id, action_class, available_tools)?
    };
    Ok(SemanticPlanStep {
        id,
        objective,
        depends_on,
        required_evidence,
        obligations,
        done_when,
        action_class,
        preferred_tools,
    })
}

pub(super) fn parse_obligations(
    object: &serde_json::Map<String, serde_json::Value>,
    step_id: &str,
    action_class: PlanActionClass,
    obligation_ids: &mut BTreeSet<String>,
) -> Result<Vec<PlanEvidenceObligation>, IpcError> {
    let values = object
        .get("obligations")
        .and_then(serde_json::Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= 8)
        .ok_or_else(|| {
            contract_error(&format!(
                "Plan step `{step_id}` must declare one to eight typed obligations"
            ))
        })?;
    let mut obligations = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let path = format!("steps[{step_id}].obligations[{index}]");
        let obligation = value
            .as_object()
            .ok_or_else(|| contract_error(&format!("{path} must be an object")))?;
        let expected = ["evidenceClass", "id", "postcondition", "roleKind"];
        if let Some(field) = obligation
            .keys()
            .find(|field| !expected.contains(&field.as_str()))
        {
            return Err(contract_error(&format!(
                "{path}.{field} is not allowed by the closed obligation contract"
            )));
        }
        if let Some(field) = expected
            .iter()
            .find(|field| !obligation.contains_key(**field))
        {
            return Err(contract_error(&format!("{path}.{field} is required")));
        }
        let id = obligation["id"]
            .as_str()
            .filter(|id| valid_obligation_id(id))
            .ok_or_else(|| {
                contract_error(&format!(
                    "{path}.id must be a bounded lower-case task-local identifier"
                ))
            })?
            .to_owned();
        if !obligation_ids.insert(id.clone()) {
            return Err(contract_error(&format!(
                "Obligation identifier `{id}` is duplicated in the semantic plan"
            )));
        }
        let role_kind = PlanEvidenceRoleKind::parse(&obligation["roleKind"], step_id)?;
        let evidence_class = PlanEvidenceClass::parse(&obligation["evidenceClass"], step_id)?;
        let postcondition =
            PlanEvidencePostcondition::parse(&obligation["postcondition"], step_id)?;
        validate_obligation_semantics(
            &path,
            action_class,
            role_kind,
            evidence_class,
            postcondition,
        )?;
        obligations.push(PlanEvidenceObligation {
            id,
            role_kind,
            evidence_class,
            postcondition,
        });
    }
    let required_effect = match action_class {
        PlanActionClass::Read | PlanActionClass::WorkspaceWrite => None,
        PlanActionClass::Compute => Some(PlanEvidencePostcondition::ComputationCompleted),
        PlanActionClass::ExternalWrite => Some(PlanEvidencePostcondition::ExternalEffectVerified),
    };
    if required_effect.is_some_and(|required| {
        !obligations
            .iter()
            .any(|obligation| obligation.postcondition == required)
    }) || (action_class == PlanActionClass::WorkspaceWrite
        && !obligations.iter().any(|obligation| {
            matches!(
                obligation.postcondition,
                PlanEvidencePostcondition::FileWriteVerified
                    | PlanEvidencePostcondition::DirectoryCreateVerified
            )
        }))
    {
        return Err(contract_error(&format!(
            "Plan step `{step_id}` must declare a target obligation with the verified postcondition for actionClass `{}`",
            action_class.as_str()
        )));
    }
    let effect_count = obligations
        .iter()
        .filter(|obligation| obligation.postcondition.is_effect())
        .count();
    if action_class != PlanActionClass::Read && effect_count != 1 {
        return Err(contract_error(&format!(
            "Plan step `{step_id}` must declare exactly one target output obligation for actionClass `{}`",
            action_class.as_str()
        )));
    }
    Ok(obligations)
}

pub(super) fn validate_obligation_semantics(
    path: &str,
    action_class: PlanActionClass,
    role_kind: PlanEvidenceRoleKind,
    evidence_class: PlanEvidenceClass,
    postcondition: PlanEvidencePostcondition,
) -> Result<(), IpcError> {
    let class_matches = match postcondition {
        PlanEvidencePostcondition::IdentityLocated
        | PlanEvidencePostcondition::FileWriteVerified
        | PlanEvidencePostcondition::DirectoryCreateVerified
        | PlanEvidencePostcondition::ExternalEffectVerified => {
            evidence_class == PlanEvidenceClass::IdentityLocation
        }
        PlanEvidencePostcondition::FormatLoaded
        | PlanEvidencePostcondition::TextFormatLoaded
        | PlanEvidencePostcondition::ImageFormatLoaded => {
            evidence_class == PlanEvidenceClass::FormatStructure
        }
        PlanEvidencePostcondition::ContentLoaded
        | PlanEvidencePostcondition::TextContentLoaded
        | PlanEvidencePostcondition::ImageContentLoaded
        | PlanEvidencePostcondition::ComputationCompleted => {
            evidence_class == PlanEvidenceClass::SemanticContent
        }
    };
    if !class_matches {
        return Err(contract_error(&format!(
            "{path}.postcondition is incompatible with its evidenceClass"
        )));
    }
    if postcondition.is_effect() && role_kind != PlanEvidenceRoleKind::Target {
        return Err(contract_error(&format!(
            "{path}.roleKind must be `target` for an effect postcondition"
        )));
    }
    let action_matches = match postcondition {
        PlanEvidencePostcondition::ComputationCompleted => action_class == PlanActionClass::Compute,
        PlanEvidencePostcondition::FileWriteVerified
        | PlanEvidencePostcondition::DirectoryCreateVerified => {
            action_class == PlanActionClass::WorkspaceWrite
        }
        PlanEvidencePostcondition::ExternalEffectVerified => {
            action_class == PlanActionClass::ExternalWrite
        }
        _ => action_class == PlanActionClass::Read || role_kind != PlanEvidenceRoleKind::Target,
    };
    if !action_matches {
        return Err(contract_error(&format!(
            "{path}.postcondition is incompatible with actionClass `{}`",
            action_class.as_str()
        )));
    }
    Ok(())
}

pub(super) fn valid_obligation_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(super) fn parse_preferred_tools(
    object: &serde_json::Map<String, serde_json::Value>,
    step_id: &str,
    action_class: PlanActionClass,
    available_tools: &BTreeSet<&str>,
) -> Result<Vec<String>, IpcError> {
    let Some(value) = object.get("preferredTools") else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .filter(|tools| !tools.is_empty() && tools.len() <= 8)
        .ok_or_else(|| {
            contract_error(&format!(
                "Plan step `{step_id}` preferredTools must contain one to eight advertised tools when present"
            ))
        })?
        .iter()
        .map(|value| {
            let tool = bounded_string(value, 1, 128).ok_or_else(|| {
                contract_error(&format!(
                    "Plan step `{step_id}` preferredTools entries must be tool names"
                ))
            })?;
            if !available_tools.contains(tool.as_str()) {
                Err(contract_error(&format!(
                    "Plan step `{step_id}` prefers `{tool}`, which is not an advertised execution tool"
                )))
            } else if !action_class.matches_tool(&tool) {
                Err(contract_error(&format!(
                    "Plan step `{step_id}` declares actionClass `{}` but prefers incompatible tool `{tool}`",
                    action_class.as_str()
                )))
            } else {
                Ok(tool)
            }
        })
        .collect()
}

pub(super) fn order_satisfiable_dependencies(
    steps: Vec<SemanticPlanStep>,
) -> Result<Vec<SemanticPlanStep>, IpcError> {
    let indices = steps
        .iter()
        .map(|step| step.id.as_str())
        .collect::<BTreeSet<_>>();
    for step in &steps {
        let mut unique = BTreeSet::new();
        for dependency in &step.depends_on {
            if !unique.insert(dependency) {
                return Err(contract_error(&format!(
                    "Plan step `{}` repeats dependency `{dependency}`",
                    step.id
                )));
            }
            if dependency == &step.id {
                return Err(contract_error(&format!(
                    "Plan step `{}` cannot depend on itself",
                    step.id
                )));
            }
            if !indices.contains(dependency.as_str()) {
                return Err(contract_error(&format!(
                    "Plan step `{}` depends on unknown step `{dependency}`",
                    step.id
                )));
            }
        }
    }

    let mut remaining = steps;
    let mut completed = BTreeSet::new();
    let mut ordered = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let Some(index) = remaining
            .iter()
            .enumerate()
            .filter(|(_, step)| {
                step.depends_on
                    .iter()
                    .all(|dependency| completed.contains(dependency))
            })
            .min_by_key(|(_, step)| step.id.as_str())
            .map(|(index, _)| index)
        else {
            let blocked = remaining
                .iter()
                .map(|step| step.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(contract_error(&format!(
                "Plan dependency graph contains a cycle among step(s): {blocked}"
            )));
        };
        let step = remaining.remove(index);
        completed.insert(step.id.clone());
        ordered.push(step);
    }
    Ok(ordered)
}
