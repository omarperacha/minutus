//! Focused plan-policy responsibility extracted from the feature coordinator.

//! Strict parsing and validation of model-supplied plans and forecasts.

use super::{
    FORECAST_TOOL, ForecastCall, ForecastScope, ForecastStep, IpcError, IpcErrorCode,
    MAX_EXPECTED_CALLS_PER_STEP, MAX_PROVISIONAL_PLAN_STEPS, PLAN_TOOL, ParsedToolCall,
    PlanActionClass, PlanContractVersion, PlanForecast, PlannedOutcome, PromptToolDefinition,
    SemanticPlanContract, is_forecastable_chat_tool, operation_category, parse_evidence_provisions,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn parse_semantic_steps(
    call: &ParsedToolCall,
    all_tools: &[PromptToolDefinition],
    contract_version: PlanContractVersion,
) -> Result<(PlanForecast, SemanticPlanContract), IpcError> {
    let available = all_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .filter(|name| is_forecastable_chat_tool(name))
        .collect::<BTreeSet<_>>();
    let contract = SemanticPlanContract::parse(
        &call.arguments,
        &available,
        MAX_PROVISIONAL_PLAN_STEPS,
        contract_version,
    )?;
    let mut expected_tools = BTreeSet::new();
    let steps = contract
        .steps
        .iter()
        .map(|step| {
            let calls = step
                .preferred_tools
                .iter()
                .map(|tool| {
                    expected_tools.insert(tool.clone());
                    ForecastCall {
                        tool: tool.clone(),
                        purpose: step.objective.clone(),
                        scope: None,
                        provides_evidence: Vec::new(),
                        completed: false,
                        completed_call_id: None,
                    }
                })
                .collect::<Vec<_>>();
            ForecastStep {
                id: step.id.clone(),
                depends_on: step.depends_on.clone(),
                purpose: step.objective.clone(),
                required_evidence: if step.obligations.is_empty() {
                    step.required_evidence
                        .iter()
                        .map(|class| class.as_str().to_owned())
                        .collect()
                } else {
                    step.obligations
                        .iter()
                        .map(|obligation| {
                            format!(
                                "{}:{}:{}:{}",
                                obligation.id,
                                obligation.role_kind.as_str(),
                                obligation.evidence_class.as_str(),
                                obligation.postcondition.as_str()
                            )
                        })
                        .collect()
                },
                completion_evidence: step.done_when.clone(),
                calls,
                action_class: Some(step.action_class),
                semantic_required_evidence: step.required_evidence.clone(),
                semantic_obligations: step.obligations.clone(),
                satisfied_obligations: BTreeMap::new(),
                semantic_complete: false,
                semantic_completion_declared: false,
                semantic_completed_call_ids: Vec::new(),
                blocked: false,
            }
        })
        .collect::<Vec<_>>();
    let estimated_calls = steps.len();
    Ok((
        PlanForecast {
            contract_version,
            estimated_calls,
            expected_tools,
            steps,
            diagnostics: Vec::new(),
        },
        contract,
    ))
}

#[expect(
    clippy::too_many_lines,
    reason = "the closed nested plan schema is validated in one fail-closed pass"
)]
pub(super) fn parse_steps(
    call: &ParsedToolCall,
    all_tools: &[PromptToolDefinition],
) -> Result<PlanForecast, IpcError> {
    let steps = array(&call.arguments, "steps")?;
    if steps.is_empty() {
        return Err(policy_error(
            "The provisional plan must contain at least one step",
        ));
    }
    if steps.len() > MAX_PROVISIONAL_PLAN_STEPS {
        return Err(policy_error(&format!(
            "The provisional plan contains {} steps; the maximum is {MAX_PROVISIONAL_PLAN_STEPS}. Merge related stages and retry",
            steps.len()
        )));
    }
    let available = all_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .filter(|name| is_forecastable_chat_tool(name))
        .collect::<BTreeSet<_>>();
    let mut ids = BTreeSet::new();
    let mut estimated_calls = 0_usize;
    let mut expected_tools = BTreeSet::new();
    let mut forecast_steps: Vec<ForecastStep> = Vec::with_capacity(steps.len());
    let mut diagnostics = Vec::new();
    for (step_index, step) in steps.iter().enumerate() {
        let object = step.as_object().ok_or_else(invalid_arguments)?;
        let keys = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if keys
            != BTreeSet::from([
                "completionEvidence",
                "expectedCalls",
                "id",
                "purpose",
                "requiredEvidence",
            ])
        {
            return Err(invalid_arguments());
        }
        let id = bounded_string(object.get("id").ok_or_else(invalid_arguments)?, 1, 32)?;
        if !ids.insert(id.clone()) {
            return Err(policy_error("Plan step identifiers must be unique"));
        }
        let purpose = bounded_string(object.get("purpose").ok_or_else(invalid_arguments)?, 1, 256)?;
        let completion_evidence = bounded_string(
            object
                .get("completionEvidence")
                .ok_or_else(invalid_arguments)?,
            1,
            256,
        )?;
        let required_evidence = object
            .get("requiredEvidence")
            .and_then(serde_json::Value::as_array)
            .filter(|values| (1..=6).contains(&values.len()))
            .ok_or_else(invalid_arguments)?
            .iter()
            .map(|value| bounded_string(value, 1, 256))
            .collect::<Result<Vec<_>, _>>()?;
        let expected = object
            .get("expectedCalls")
            .and_then(serde_json::Value::as_array)
            .filter(|calls| !calls.is_empty() && calls.len() <= MAX_EXPECTED_CALLS_PER_STEP)
            .ok_or_else(invalid_arguments)?;
        let mut forecast_calls = Vec::with_capacity(expected.len());
        for (call_index, expected_call) in expected.iter().enumerate() {
            let expected_call = expected_call.as_object().ok_or_else(invalid_arguments)?;
            let keys = expected_call
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            if keys != BTreeSet::from(["purpose", "tool"])
                && keys != BTreeSet::from(["purpose", "scope", "tool"])
                && keys != BTreeSet::from(["providesEvidence", "purpose", "tool"])
                && keys != BTreeSet::from(["providesEvidence", "purpose", "scope", "tool"])
            {
                return Err(invalid_arguments());
            }
            let tool = bounded_string(
                expected_call.get("tool").ok_or_else(invalid_arguments)?,
                1,
                128,
            )?;
            let definition = all_tools.iter().find(|definition| definition.name == tool);
            let advertised = definition.is_some();
            if advertised && !is_forecastable_chat_tool(&tool) {
                return Err(policy_error(&format!(
                    "Plan step `{id}` predicts `{tool}` at steps[{step_index}].expectedCalls[{call_index}].tool, which is an orchestration control operation rather than forecastable task work. Remove this entire step if it represents reasoning, formatting, reporting or the final assistant response; do not invent an execution call to preserve it"
                )));
            }
            if !available.contains(tool.as_str()) {
                return Err(policy_error(&format!(
                    "Plan step `{id}` predicts `{tool}` at steps[{step_index}].expectedCalls[{call_index}].tool, which is not an advertised forecastable execution tool. Choose a tool from the advertised expectedCalls.tool values, or remove this entire step if it represents reasoning, formatting, reporting or the final assistant response; do not invent an execution call to preserve it"
                )));
            }
            expected_tools.insert(tool.clone());
            let purpose = bounded_string(
                expected_call.get("purpose").ok_or_else(invalid_arguments)?,
                1,
                256,
            )?;
            let scope = match expected_call.get("scope") {
                Some(value) => match parse_forecast_scope(
                    value,
                    definition.ok_or_else(invalid_arguments)?,
                    &id,
                    step_index,
                    call_index,
                ) {
                    Ok(scope) => Some(scope),
                    Err(error) => {
                        diagnostics.push(error.message);
                        None
                    }
                },
                None => None,
            };
            let provides_evidence = match parse_evidence_provisions(expected_call, &tool, &id) {
                Ok(provisions) => provisions,
                Err(error) => {
                    diagnostics.push(error.message);
                    Vec::new()
                }
            };
            forecast_calls.push(ForecastCall {
                tool,
                purpose,
                scope,
                provides_evidence,
                completed: false,
                completed_call_id: None,
            });
        }
        estimated_calls = estimated_calls.saturating_add(expected.len());
        forecast_steps.push(ForecastStep {
            id,
            depends_on: forecast_steps
                .last()
                .map(|step| vec![step.id.clone()])
                .unwrap_or_default(),
            purpose,
            required_evidence,
            completion_evidence,
            calls: forecast_calls,
            action_class: None,
            semantic_required_evidence: Vec::new(),
            semantic_obligations: Vec::new(),
            satisfied_obligations: BTreeMap::new(),
            semantic_complete: false,
            semantic_completion_declared: false,
            semantic_completed_call_ids: Vec::new(),
            blocked: false,
        });
    }
    Ok(PlanForecast {
        contract_version: PlanContractVersion::LegacyForecastV1,
        estimated_calls,
        expected_tools,
        steps: forecast_steps,
        diagnostics,
    })
}

pub(super) fn parse_forecast_scope(
    value: &serde_json::Value,
    tool: &PromptToolDefinition,
    step_id: &str,
    step_index: usize,
    call_index: usize,
) -> Result<ForecastScope, IpcError> {
    let object = value.as_object().ok_or_else(invalid_arguments)?;
    if object.keys().map(String::as_str).collect::<BTreeSet<_>>()
        != BTreeSet::from(["argument", "value"])
    {
        return Err(invalid_arguments());
    }
    let argument = bounded_string(object.get("argument").ok_or_else(invalid_arguments)?, 1, 64)?;
    if !argument
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(invalid_arguments());
    }
    let value = bounded_string(object.get("value").ok_or_else(invalid_arguments)?, 1, 4096)?;
    let argument_schema = tool.parameters["properties"]
        .get(&argument)
        .ok_or_else(|| {
            policy_error(&format!(
                "Plan step `{step_id}` scopes `{}` by unknown argument `{argument}` at steps[{step_index}].expectedCalls[{call_index}].scope.argument",
                tool.name
            ))
        })?;
    if !schema_can_contain_string(argument_schema) {
        return Err(policy_error(&format!(
            "Plan step `{step_id}` cannot scope `{}` by `{argument}` because that argument contains no string discriminator",
            tool.name
        )));
    }
    if serde_json::from_str::<serde_json::Value>(&value)
        .is_ok_and(|parsed| parsed.is_array() || parsed.is_object())
    {
        return Err(policy_error(&format!(
            "Plan step `{step_id}` scopes `{}` with a serialized JSON object or array. scope.value must be one atomic string discriminator contained in the actual `{argument}` argument; do not encode the complete argument as a string",
            tool.name
        )));
    }
    Ok(ForecastScope { argument, value })
}

pub(super) fn schema_can_contain_string(schema: &serde_json::Value) -> bool {
    let directly_string = match schema.get("type") {
        Some(serde_json::Value::String(kind)) => kind == "string",
        Some(serde_json::Value::Array(kinds)) => kinds.iter().any(|kind| kind == "string"),
        _ => schema
            .get("enum")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|values| values.iter().any(serde_json::Value::is_string)),
    };
    directly_string
        || schema.get("items").is_some_and(schema_can_contain_string)
        || schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|properties| properties.values().any(schema_can_contain_string))
        || ["oneOf", "anyOf", "allOf"].iter().any(|keyword| {
            schema
                .get(keyword)
                .and_then(serde_json::Value::as_array)
                .is_some_and(|variants| variants.iter().any(schema_can_contain_string))
        })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ExecutionForecast {
    pub(super) entries: Vec<ExecutionForecastEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ExecutionForecastEntry {
    pub(super) step_id: String,
    pub(super) category: String,
    pub(super) purpose: String,
    pub(super) operations: Vec<String>,
}

#[expect(
    clippy::too_many_lines,
    reason = "one closed traversal reports the first exact nested forecast defect without partially admitting entries"
)]
pub(super) fn parse_execution_forecast(
    call: &ParsedToolCall,
    contract: &SemanticPlanContract,
    contract_version: PlanContractVersion,
    all_tools: &[PromptToolDefinition],
) -> Result<ExecutionForecast, IpcError> {
    let entries = array(&call.arguments, "entries")?;
    if entries.len() != contract.steps.len() {
        return Err(forecast_error(
            "$.entries",
            &format!(
                "must contain exactly one entry for each of the {} semantic plan steps",
                contract.steps.len()
            ),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut parsed = Vec::with_capacity(entries.len());
    for (index, value) in entries.iter().enumerate() {
        let path = format!("$.entries[{index}]");
        let entry = value
            .as_object()
            .ok_or_else(|| forecast_error(&path, "must be an object"))?;
        let evidence_field = if contract_version.uses_typed_obligations() {
            "requiredObligations"
        } else {
            "requiredEvidence"
        };
        let expected = [
            "category",
            "dependsOn",
            "postcondition",
            "purpose",
            evidence_field,
            "routes",
            "stepId",
        ];
        validate_nested_keys(entry, &expected, &path)?;
        let step_id = nested_string(entry, "stepId", &path)?;
        if !seen.insert(step_id.to_owned()) {
            return Err(forecast_error(
                &format!("{path}.stepId"),
                "duplicates an earlier forecast entry",
            ));
        }
        let step = contract
            .steps
            .iter()
            .find(|candidate| candidate.id == step_id)
            .ok_or_else(|| {
                forecast_error(
                    &format!("{path}.stepId"),
                    "must name a step in the accepted semantic plan",
                )
            })?;
        let dependencies = nested_string_array(entry, "dependsOn", &path)?;
        if as_set(&dependencies) != as_set(&step.depends_on) {
            return Err(forecast_error(
                &format!("{path}.dependsOn"),
                "must exactly preserve the semantic step dependencies",
            ));
        }
        let evidence = nested_string_array(entry, evidence_field, &path)?;
        let required = if contract_version.uses_typed_obligations() {
            step.obligations
                .iter()
                .map(|obligation| obligation.id.clone())
                .collect::<Vec<_>>()
        } else {
            step.required_evidence
                .iter()
                .map(|class| class.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        if as_set(&evidence) != as_set(&required) {
            return Err(forecast_error(
                &format!("{path}.{evidence_field}"),
                if contract_version.uses_typed_obligations() {
                    "must exactly cover the semantic step's declared obligation IDs"
                } else {
                    "must exactly cover the semantic step's declared evidence classes"
                },
            ));
        }
        let category = nested_string(entry, "category", &path)?;
        let purpose = nested_string(entry, "purpose", &path)?;
        let postcondition = nested_string(entry, "postcondition", &path)?;
        if !postcondition_matches_action(postcondition, step.action_class) {
            return Err(forecast_error(
                &format!("{path}.postcondition"),
                &format!(
                    "is incompatible with actionClass `{}`",
                    step.action_class.as_str()
                ),
            ));
        }
        let routes = entry
            .get("routes")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| forecast_error(&format!("{path}.routes"), "must be an array"))?;
        if routes.is_empty() || routes.len() > 3 {
            return Err(forecast_error(
                &format!("{path}.routes"),
                "must contain one to three small alternatives",
            ));
        }
        let mut operations = Vec::with_capacity(routes.len());
        for (route_index, value) in routes.iter().enumerate() {
            let route_path = format!("{path}.routes[{route_index}]");
            let route = value
                .as_object()
                .ok_or_else(|| forecast_error(&route_path, "must be an object"))?;
            validate_nested_keys(
                route,
                &["knownArguments", "operation", "unresolvedArguments"],
                &route_path,
            )?;
            let operation = nested_string(route, "operation", &route_path)?;
            if operation == PLAN_TOOL
                || operation == FORECAST_TOOL
                || !step.action_class.matches_tool(operation)
            {
                return Err(forecast_error(
                    &format!("{route_path}.operation"),
                    &format!(
                        "must be a tool-backed `{}` operation and cannot forecast an orchestration control",
                        step.action_class.as_str()
                    ),
                ));
            }
            let expected_category = operation_category(operation).ok_or_else(|| {
                forecast_error(&format!("{route_path}.operation"), "is not forecastable")
            })?;
            if category != expected_category {
                return Err(forecast_error(
                    &format!("{path}.category"),
                    &format!("must be `{expected_category}` for operation `{operation}`"),
                ));
            }
            if !postcondition_matches_operation(postcondition, operation) {
                return Err(forecast_error(
                    &format!("{path}.postcondition"),
                    &format!("does not describe the verified result of operation `{operation}`"),
                ));
            }
            validate_forecast_arguments(route, &route_path, operation, all_tools)?;
            if !operations.iter().any(|candidate| candidate == operation) {
                operations.push(operation.to_owned());
            }
        }
        parsed.push(ExecutionForecastEntry {
            step_id: step_id.to_owned(),
            category: category.to_owned(),
            purpose: purpose.to_owned(),
            operations,
        });
    }
    Ok(ExecutionForecast { entries: parsed })
}

pub(super) fn validate_forecast_arguments(
    route: &serde_json::Map<String, serde_json::Value>,
    path: &str,
    operation: &str,
    all_tools: &[PromptToolDefinition],
) -> Result<(), IpcError> {
    let definition = all_tools
        .iter()
        .find(|tool| tool.name == operation)
        .ok_or_else(|| forecast_error(&format!("{path}.operation"), "is unavailable"))?;
    let known = nested_string_array(route, "knownArguments", path)?;
    let unresolved = nested_string_array(route, "unresolvedArguments", path)?;
    let known_set = as_set(&known);
    let unresolved_set = as_set(&unresolved);
    if let Some(duplicate) = known_set.intersection(&unresolved_set).next() {
        return Err(forecast_error(
            &format!("{path}.unresolvedArguments"),
            &format!("argument `{duplicate}` cannot be both known and unresolved"),
        ));
    }
    let properties = definition.parameters["properties"].as_object();
    for argument in known_set.union(&unresolved_set) {
        if !properties.is_some_and(|properties| properties.contains_key(*argument)) {
            return Err(forecast_error(
                &format!("{path}.knownArguments"),
                &format!("`{argument}` is not an advertised top-level argument of `{operation}`"),
            ));
        }
    }
    let required = definition.parameters["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str);
    for argument in required {
        if !known_set.contains(argument) && !unresolved_set.contains(argument) {
            return Err(forecast_error(
                &format!("{path}.knownArguments"),
                &format!(
                    "required argument `{argument}` must be classified as known or unresolved"
                ),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_nested_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    expected: &[&str],
    path: &str,
) -> Result<(), IpcError> {
    if let Some(unexpected) = object.keys().find(|key| !expected.contains(&key.as_str())) {
        return Err(forecast_error(
            &format!("{path}.{unexpected}"),
            "is not allowed by the closed forecast schema",
        ));
    }
    if let Some(missing) = expected.iter().find(|key| !object.contains_key(**key)) {
        return Err(forecast_error(&format!("{path}.{missing}"), "is required"));
    }
    Ok(())
}

pub(super) fn nested_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
    path: &str,
) -> Result<&'a str, IpcError> {
    object
        .get(name)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| forecast_error(&format!("{path}.{name}"), "must be a non-empty string"))
}

pub(super) fn nested_string_array(
    object: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    path: &str,
) -> Result<Vec<String>, IpcError> {
    let values = object
        .get(name)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| forecast_error(&format!("{path}.{name}"), "must be an array"))?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    forecast_error(
                        &format!("{path}.{name}[{index}]"),
                        "must be a non-empty string",
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut unique = BTreeSet::new();
    if let Some((index, duplicate)) = values
        .iter()
        .enumerate()
        .find(|(_, value)| !unique.insert(value.as_str()))
    {
        return Err(forecast_error(
            &format!("{path}.{name}[{index}]"),
            &format!("duplicates `{duplicate}`"),
        ));
    }
    Ok(values)
}

pub(super) fn as_set(values: &[String]) -> BTreeSet<&str> {
    values.iter().map(String::as_str).collect()
}

pub(super) fn postcondition_matches_action(postcondition: &str, action: PlanActionClass) -> bool {
    match action {
        PlanActionClass::Read => matches!(postcondition, "evidence_ready" | "content_loaded"),
        PlanActionClass::Compute => postcondition == "computation_completed",
        PlanActionClass::WorkspaceWrite => matches!(
            postcondition,
            "file_write_verified" | "directory_create_verified"
        ),
        PlanActionClass::ExternalWrite => postcondition == "external_effect_verified",
    }
}

pub(super) fn postcondition_matches_operation(postcondition: &str, operation: &str) -> bool {
    match operation {
        "filesystem.list"
        | "filesystem.find_paths"
        | "filesystem.stat"
        | "filesystem.search"
        | "filesystem.scan"
        | "web.search" => postcondition == "evidence_ready",
        "filesystem.collect_evidence"
        | "filesystem.read_text"
        | "filesystem.inspect_image"
        | "web.retrieve" => {
            matches!(postcondition, "evidence_ready" | "content_loaded")
        }
        "computation.run" | "computation.run_project" => postcondition == "computation_completed",
        "filesystem.create_directory" => postcondition == "directory_create_verified",
        "filesystem.write_text"
        | "filesystem.replace_text"
        | "filesystem.write_delimited_table"
        | "orchestration.transform_artifact" => postcondition == "file_write_verified",
        _ => false,
    }
}

pub(super) fn forecast_error(path: &str, message: &str) -> IpcError {
    policy_error(&format!(
        "Invalid execution forecast at `{path}`: {message}"
    ))
}

pub(super) fn array<'a>(
    arguments: &'a std::collections::BTreeMap<String, serde_json::Value>,
    name: &str,
) -> Result<&'a Vec<serde_json::Value>, IpcError> {
    arguments
        .get(name)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(invalid_arguments)
}

pub(super) fn planned_outcome(call: &ParsedToolCall) -> Result<PlannedOutcome, IpcError> {
    match string(&call.arguments, "outcome")? {
        "answer" => Ok(PlannedOutcome::Answer),
        "create_file" => Ok(PlannedOutcome::CreateFile),
        "modify_file" => Ok(PlannedOutcome::ModifyFile),
        "other" => Ok(PlannedOutcome::Other),
        _ => Err(invalid_arguments()),
    }
}

pub(super) fn plan_contract_metadata(
    call: &ParsedToolCall,
) -> Result<(PlanContractVersion, PlannedOutcome), IpcError> {
    let contract_version = PlanContractVersion::from_call(call)?;
    // V6 performs one complete structural pass in SemanticPlanContract::parse
    // so the model receives every independently repairable path at once.
    if contract_version != PlanContractVersion::SemanticExecutionV6 {
        bounded_string(
            call.arguments.get("target").ok_or_else(invalid_arguments)?,
            1,
            512,
        )?;
        bounded_string_array(&call.arguments, "successCriteria", 1, 8)?;
        bounded_string_array(&call.arguments, "assumptions", 0, 6)?;
        if contract_version.is_semantic() {
            bounded_string_array(&call.arguments, "constraints", 0, 8)?;
        }
    }
    Ok((contract_version, planned_outcome(call)?))
}

pub(super) fn string<'a>(
    arguments: &'a std::collections::BTreeMap<String, serde_json::Value>,
    name: &str,
) -> Result<&'a str, IpcError> {
    arguments
        .get(name)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid_arguments)
}

pub(super) fn bounded_string_array(
    arguments: &std::collections::BTreeMap<String, serde_json::Value>,
    name: &str,
    min: usize,
    max: usize,
) -> Result<(), IpcError> {
    bounded_value_string_array(arguments.get(name).ok_or_else(invalid_arguments)?, min, max)
}

pub(super) fn bounded_value_string_array(
    value: &serde_json::Value,
    min: usize,
    max: usize,
) -> Result<(), IpcError> {
    let values = value
        .as_array()
        .filter(|values| (min..=max).contains(&values.len()))
        .ok_or_else(invalid_arguments)?;
    values
        .iter()
        .try_for_each(|value| bounded_string(value, 1, 256).map(|_| ()))
}

pub(super) fn bounded_string(
    value: &serde_json::Value,
    min: usize,
    max: usize,
) -> Result<String, IpcError> {
    value
        .as_str()
        .filter(|value| (min..=max).contains(&value.len()))
        .map(str::to_owned)
        .ok_or_else(invalid_arguments)
}

pub(super) fn invalid_arguments() -> IpcError {
    IpcError::invalid_request("The model returned invalid provisional plan arguments")
}

pub(super) fn policy_error(message: &str) -> IpcError {
    IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: message.to_owned(),
        retryable: true,
    }
}
