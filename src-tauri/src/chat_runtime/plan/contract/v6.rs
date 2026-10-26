use super::{
    BTreeMap, BTreeSet, IpcError, PlanActionClass, PlanContractVersion,
    SEMANTIC_PLAN_CONTRACT_VERSION, SemanticPlanContract, SemanticPlanStep, bounded_object_string,
    bounded_object_string_array, bounded_string, bounded_string_array, contract_error,
    order_satisfiable_dependencies,
};

pub(super) fn parse_v6_contract(
    arguments: &BTreeMap<String, serde_json::Value>,
    available_tools: &BTreeSet<&str>,
    maximum_steps: usize,
) -> Result<SemanticPlanContract, IpcError> {
    validate_v6_contract_structure(arguments, available_tools, maximum_steps)?;
    let target = arguments
        .get("target")
        .and_then(|value| bounded_string(value, 1, 512))
        .ok_or_else(|| contract_error("`target` must identify the requested deliverable"))?;
    let constraints = bounded_string_array(arguments, "constraints", 0, 8)?;
    let success_criteria = bounded_string_array(arguments, "successCriteria", 1, 8)?;
    let assumptions = bounded_string_array(arguments, "assumptions", 0, 8)?;
    let values = arguments
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .filter(|steps| !steps.is_empty() && steps.len() <= maximum_steps)
        .ok_or_else(|| contract_error("The semantic plan must contain one to sixteen stages"))?;
    let mut ids = BTreeSet::new();
    let mut steps = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let object = value.as_object().ok_or_else(|| {
            contract_error(&format!("Plan stage at steps[{index}] must be an object"))
        })?;
        let id = bounded_object_string(object, "id", 1, 32, index)?;
        if !ids.insert(id.clone()) {
            return Err(contract_error(&format!(
                "Plan stage identifier `{id}` is duplicated"
            )));
        }
        let objective = bounded_object_string(object, "objective", 1, 256, index)?;
        let done_when = bounded_object_string(object, "doneWhen", 1, 256, index)?;
        let action_class = PlanActionClass::parse(
            object
                .get("actionClass")
                .ok_or_else(|| contract_error("actionClass is required"))?,
            &id,
            PlanContractVersion::SemanticExecutionV6,
        )?;
        let depends_on = if object.contains_key("dependsOn") {
            bounded_object_string_array(object, "dependsOn", 0, 15, &id)?
        } else {
            Vec::new()
        };
        let suggested_tools = object
            .get("suggestedTools")
            .and_then(serde_json::Value::as_array)
            .filter(|tools| !tools.is_empty() && tools.len() <= 8)
            .ok_or_else(|| {
                contract_error(&format!(
                    "Plan stage `{id}` must suggest one to eight advertised tools"
                ))
            })?
            .iter()
            .map(|value| {
                let tool = bounded_string(value, 1, 128).ok_or_else(|| {
                    contract_error(&format!(
                        "Plan stage `{id}` suggestedTools entries must be tool names"
                    ))
                })?;
                if !available_tools.contains(tool.as_str()) {
                    return Err(contract_error(&format!(
                        "Plan stage `{id}` suggests unavailable tool `{tool}`"
                    )));
                }
                Ok(tool)
            })
            .collect::<Result<Vec<_>, _>>()?;
        steps.push(SemanticPlanStep {
            id,
            objective,
            depends_on,
            required_evidence: Vec::new(),
            obligations: Vec::new(),
            done_when,
            action_class,
            preferred_tools: suggested_tools,
        });
    }
    let steps = order_satisfiable_dependencies(steps)?;
    Ok(SemanticPlanContract {
        target,
        constraints,
        success_criteria,
        assumptions,
        steps,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "one complete structural pass returns every independently repairable v6 defect"
)]
pub(super) fn validate_v6_contract_structure(
    arguments: &BTreeMap<String, serde_json::Value>,
    available_tools: &BTreeSet<&str>,
    maximum_steps: usize,
) -> Result<(), IpcError> {
    let mut defects = Vec::new();
    let allowed = BTreeSet::from([
        "assumptions",
        "constraints",
        "contractVersion",
        "mode",
        "outcome",
        "steps",
        "successCriteria",
        "target",
    ]);
    for field in arguments
        .keys()
        .filter(|field| !allowed.contains(field.as_str()))
    {
        defects.push(v6_defect(
            "unknown_field",
            &format!("/{field}"),
            arguments.get(field).unwrap_or(&serde_json::Value::Null),
            &serde_json::json!({"remove":true}),
        ));
    }
    validate_v6_string(arguments, "target", 1, 512, &mut defects);
    validate_v6_string_array(arguments, "constraints", 0, 8, &mut defects);
    validate_v6_string_array(arguments, "successCriteria", 1, 8, &mut defects);
    validate_v6_string_array(arguments, "assumptions", 0, 6, &mut defects);

    let Some(values) = arguments.get("steps").and_then(serde_json::Value::as_array) else {
        defects.push(v6_defect(
            "invalid_array",
            "/steps",
            arguments.get("steps").unwrap_or(&serde_json::Value::Null),
            &serde_json::json!({
                "type":"array",
                "minItems":1,
                "maxItems":maximum_steps,
                "items":"stage objects",
                "neverStringify":true
            }),
        ));
        return Err(v6_contract_error(&defects));
    };
    if values.is_empty() || values.len() > maximum_steps {
        defects.push(v6_defect(
            "invalid_array_length",
            "/steps",
            &serde_json::json!({"itemCount":values.len()}),
            &serde_json::json!({"minItems":1,"maxItems":maximum_steps}),
        ));
    }

    let stage_fields = BTreeSet::from([
        "actionClass",
        "dependsOn",
        "doneWhen",
        "id",
        "objective",
        "suggestedTools",
    ]);
    let mut ids = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        let path = format!("/steps/{index}");
        let Some(object) = value.as_object() else {
            defects.push(v6_defect(
                "invalid_stage",
                &path,
                value,
                &serde_json::json!({
                    "type":"object",
                    "required":stage_fields.iter().filter(|field| **field != "dependsOn").collect::<Vec<_>>()
                }),
            ));
            continue;
        };
        for field in object
            .keys()
            .filter(|field| !stage_fields.contains(field.as_str()))
        {
            defects.push(v6_defect(
                "unknown_field",
                &format!("{path}/{field}"),
                object.get(field).unwrap_or(&serde_json::Value::Null),
                &serde_json::json!({"remove":true}),
            ));
        }
        let id = object.get("id").and_then(serde_json::Value::as_str);
        if id.is_none_or(|id| id.is_empty() || id.len() > 32) {
            defects.push(v6_defect(
                "invalid_string",
                &format!("{path}/id"),
                object.get("id").unwrap_or(&serde_json::Value::Null),
                &serde_json::json!({"type":"string","minLength":1,"maxLength":32}),
            ));
        } else if !ids.insert(id.unwrap_or_default()) {
            defects.push(v6_defect(
                "duplicate_stage_id",
                &format!("{path}/id"),
                object.get("id").unwrap_or(&serde_json::Value::Null),
                &serde_json::json!({"uniqueAcrossPlan":true}),
            ));
        }
        for field in ["objective", "doneWhen"] {
            let valid = object
                .get(field)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !text.is_empty() && text.len() <= 256);
            if !valid {
                defects.push(v6_defect(
                    "invalid_string",
                    &format!("{path}/{field}"),
                    object.get(field).unwrap_or(&serde_json::Value::Null),
                    &serde_json::json!({"type":"string","minLength":1,"maxLength":256}),
                ));
            }
        }
        if object.contains_key("dependsOn") {
            validate_v6_object_string_array(object, "dependsOn", 0, 15, &path, &mut defects);
        }
        match object
            .get("actionClass")
            .and_then(serde_json::Value::as_str)
        {
            Some("read" | "compute" | "workspace_write" | "external_write") => {}
            _ => {
                defects.push(v6_defect(
                    "invalid_action_class",
                    &format!("{path}/actionClass"),
                    object
                        .get("actionClass")
                        .unwrap_or(&serde_json::Value::Null),
                    &serde_json::json!(["read", "compute", "workspace_write", "external_write"]),
                ));
            }
        }
        let suggested =
            validate_v6_object_string_array(object, "suggestedTools", 1, 8, &path, &mut defects);
        if let Some(suggested) = suggested {
            for (tool_index, tool) in suggested.iter().enumerate() {
                if !available_tools.contains(tool.as_str()) {
                    // Availability is structural; the planning label cannot
                    // prescribe a semantically unrelated replacement tool.
                    let repair = serde_json::json!({
                        "availableTools": available_tools,
                        "guidance": "Choose an available tool that serves the objective; omit stages that only ask the user or report results."
                    });
                    defects.push(v6_defect(
                        "invalid_suggested_tool",
                        &format!("{path}/suggestedTools/{tool_index}"),
                        &serde_json::json!(tool),
                        &repair,
                    ));
                }
            }
        }
    }
    if defects.is_empty() {
        Ok(())
    } else {
        Err(v6_contract_error(&defects))
    }
}

pub(super) fn validate_v6_string(
    arguments: &BTreeMap<String, serde_json::Value>,
    field: &str,
    min: usize,
    max: usize,
    defects: &mut Vec<serde_json::Value>,
) {
    let value = arguments.get(field).unwrap_or(&serde_json::Value::Null);
    if value
        .as_str()
        .is_none_or(|text| !(min..=max).contains(&text.len()))
    {
        defects.push(v6_defect(
            "invalid_string",
            &format!("/{field}"),
            value,
            &serde_json::json!({"type":"string","minLength":min,"maxLength":max}),
        ));
    }
}

pub(super) fn validate_v6_string_array(
    arguments: &BTreeMap<String, serde_json::Value>,
    field: &str,
    min: usize,
    max: usize,
    defects: &mut Vec<serde_json::Value>,
) {
    let value = arguments.get(field).unwrap_or(&serde_json::Value::Null);
    let valid = value.as_array().is_some_and(|items| {
        (min..=max).contains(&items.len())
            && items.iter().all(|item| {
                item.as_str()
                    .is_some_and(|text| !text.is_empty() && text.len() <= 256)
            })
    });
    if !valid {
        defects.push(v6_defect(
            "invalid_array",
            &format!("/{field}"),
            value,
            &serde_json::json!({
                "type":"array",
                "minItems":min,
                "maxItems":max,
                "items":{"type":"string","minLength":1,"maxLength":256},
                "neverStringify":true
            }),
        ));
    }
}

pub(super) fn validate_v6_object_string_array(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    min: usize,
    max: usize,
    path: &str,
    defects: &mut Vec<serde_json::Value>,
) -> Option<Vec<String>> {
    let value = object.get(field).unwrap_or(&serde_json::Value::Null);
    let Some(items) = value
        .as_array()
        .filter(|items| (min..=max).contains(&items.len()))
    else {
        defects.push(v6_defect(
            "invalid_array",
            &format!("{path}/{field}"),
            value,
            &serde_json::json!({
                "type":"array",
                "minItems":min,
                "maxItems":max,
                "items":{"type":"string","minLength":1},
                "neverStringify":true
            }),
        ));
        return None;
    };
    let parsed = items
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>();
    if parsed.is_none() {
        defects.push(v6_defect(
            "invalid_array_item",
            &format!("{path}/{field}"),
            value,
            &serde_json::json!({"items":{"type":"string","minLength":1}}),
        ));
    }
    parsed
}

pub(super) fn v6_defect(
    code: &str,
    path: &str,
    received: &serde_json::Value,
    permitted: &serde_json::Value,
) -> serde_json::Value {
    let received = match received.as_str() {
        Some(text) if text.len() > 256 => {
            serde_json::json!({"type":"string","length":text.len()})
        }
        _ => received.clone(),
    };
    serde_json::json!({"code":code,"path":path,"received":received,"permitted":permitted})
}

pub(super) fn v6_contract_error(defects: &[serde_json::Value]) -> IpcError {
    contract_error(&format!(
        "{} plan defects: {}",
        SEMANTIC_PLAN_CONTRACT_VERSION,
        serde_json::json!({
            "defects":defects,
            "repair":"Preserve all unrelated plan content. Replace only the reported paths. Arrays must be native JSON arrays, never quoted or JSON-stringified."
        })
    ))
}
