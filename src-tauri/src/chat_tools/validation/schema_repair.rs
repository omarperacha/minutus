use super::{IpcError, ParsedToolCall, PromptToolDefinition};

#[cfg(test)]
mod tests;

#[derive(Debug, Eq, PartialEq)]
struct SchemaDefect {
    path: String,
    expected: String,
}

/// Enriches a broker-safe validation failure with one exact, bounded correction.
/// Schema mismatches identify the first deterministic JSON path; semantic
/// failures retain `$` as the exact argument-document boundary. The full schema
/// remains in the advertised tool definition and is not duplicated into history.
pub(crate) fn exact_argument_repair_error(
    call: &ParsedToolCall,
    catalogue: &[PromptToolDefinition],
    error: &IpcError,
) -> IpcError {
    let Some(tool) = catalogue.iter().find(|tool| tool.name == call.name) else {
        return error.clone();
    };
    let value = serde_json::Value::Object(
        call.arguments
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    if call.name == "x402.request"
        && let Some(repair) = x402_repair_error(call, error)
    {
        return repair;
    }
    if call.name == "filesystem.write_delimited_table"
        && let Some(repair) = delimited_table_repair_error(call, tool, error, &value)
    {
        return repair;
    }
    let defect =
        schema_defect(&tool.parameters, &value, "$".to_owned()).unwrap_or_else(|| SchemaDefect {
            path: "$".to_owned(),
            expected: "arguments satisfying the operation's semantic constraints".to_owned(),
        });
    let output_contract = computation_output_repair(call, tool);
    IpcError {
        code: error.code.clone(),
        message: format!(
            "Operation `{}` has invalid arguments at JSON path `{}`; expected {}. Validation detail: {}. Repair this path and every other defect reported in the validation detail using the advertised tool schema; preserve all unrelated valid arguments.{}",
            call.name, defect.path, defect.expected, error.message, output_contract
        ),
        retryable: error.retryable,
    }
}

fn computation_output_repair(call: &ParsedToolCall, tool: &PromptToolDefinition) -> String {
    if !matches!(
        call.name.as_str(),
        "computation.run" | "computation.run_project"
    ) {
        return String::new();
    }
    let Some(schema) = tool
        .parameters
        .get("properties")
        .and_then(|p| p.get("output"))
    else {
        return String::new();
    };
    if call
        .arguments
        .get("output")
        .is_some_and(|value| schema_defect(schema, value, "$.output".to_owned()).is_none())
    {
        return String::new();
    }
    // This small, host-owned subtree is repeated deliberately: fixing only the
    // first path hid simultaneous missing bounds and invalid output kinds.
    format!(
        " Repair the entire required nested `output` object together. Its complete contract is {schema}. Numeric results use kind `json`, not `number`. Generic scalar-output example: {{\"kind\":\"json\",\"maxBytes\":128,\"maxItems\":1}}; choose bounds sufficient for the actual result. Preserve valid `source` and other arguments."
    )
}

fn x402_repair_error(call: &ParsedToolCall, error: &IpcError) -> Option<IpcError> {
    let operation = call.arguments.get("operation")?.as_str()?;
    let (path, correction) = if call.arguments.contains_key("quote")
        || call.arguments.contains_key("request")
    {
        (
            "$",
            "The legacy Flowzed quote shape is obsolete. Preserve the user's resource choices, run standard Bazaar discovery if needed, then use `operation: \"purchase\"`, the returned `resourceRef`, and one schema-valid `input` object. The host generates only fields advertised in `hostGeneratedFields`.",
        )
    } else {
        match operation {
            "purchase" if !call.arguments.contains_key("resourceRef") => (
                "$.resourceRef",
                "Preserve `$.input` and `$.reason`; copy the exact active x402 `resourceRef` into `$.resourceRef`. Do not add `$.url` and do not rediscover.",
            ),
            "purchase" if !call.arguments.contains_key("input") => (
                "$.input",
                "Preserve `$.resourceRef` and `$.reason`; add one object at `$.input` satisfying the discovered resource's `inputSchema`. Omit every field listed in `hostGeneratedFields`. Do not add `$.url`.",
            ),
            "purchase" if call.arguments.contains_key("url") => (
                "$.url",
                "Remove `$.url`; a discovered purchase is bound by `$.resourceRef`. Preserve `$.resourceRef`, `$.input`, and `$.reason`.",
            ),
            "purchase_url" if !call.arguments.contains_key("url") => (
                "$.url",
                "Add the exact operator-supplied protected-resource URL at `$.url`. Preserve `$.operation` and `$.reason`; do not add `$.resourceRef` or `$.input`.",
            ),
            "discover" if !call.arguments.contains_key("url") => (
                "$.url",
                "Add the exact operator-supplied Bazaar discovery URL at `$.url`. Preserve `$.operation` and `$.reason`; do not add `$.resourceRef` or `$.input`.",
            ),
            _ => return None,
        }
    };
    Some(IpcError {
        code: error.code.clone(),
        message: format!(
            "Operation `x402.request` has invalid arguments at JSON path `{path}`. {correction} Validation detail: {}",
            error.message
        ),
        retryable: true,
    })
}

fn delimited_table_repair_error(
    call: &ParsedToolCall,
    tool: &PromptToolDefinition,
    error: &IpcError,
    value: &serde_json::Value,
) -> Option<IpcError> {
    const MAX_REPORTED_DEFECTS: usize = 256;
    let object = value.as_object()?;
    let properties = tool.parameters.get("properties")?.as_object()?;
    let mut defects = Vec::new();
    if let Some(required) = tool
        .parameters
        .get("required")
        .and_then(serde_json::Value::as_array)
    {
        for name in required.iter().filter_map(serde_json::Value::as_str) {
            if !object.contains_key(name) {
                defects.push(serde_json::json!({
                    "code": "missing_required_field",
                    "path": json_field_path("$", name),
                    "permitted": "required top-level field"
                }));
            }
        }
    }
    for (name, supplied) in object {
        if name != "evidence" && !properties.contains_key(name) {
            defects.push(serde_json::json!({
                "code": "unexpected_field",
                "path": json_field_path("$", name),
                "received": bounded_received_value(supplied),
                "permitted": properties.keys().cloned().collect::<Vec<_>>()
            }));
        }
    }
    for (name, supplied) in object {
        let Some(schema) = properties.get(name) else {
            continue;
        };
        if matches!(name.as_str(), "columns" | "rows") {
            continue;
        }
        if let Some(defect) = schema_defect(schema, supplied, json_field_path("$", name)) {
            defects.push(serde_json::json!({
                "code": "invalid_field",
                "path": defect.path,
                "received": bounded_received_value(supplied),
                "permitted": defect.expected
            }));
        }
    }
    append_delimited_array_defects(object, properties, &mut defects);
    append_row_length_defect(object, &mut defects, MAX_REPORTED_DEFECTS);
    if defects.len() > MAX_REPORTED_DEFECTS {
        let omitted = defects.len().saturating_sub(MAX_REPORTED_DEFECTS);
        defects.truncate(MAX_REPORTED_DEFECTS);
        defects.push(serde_json::json!({
            "code": "additional_defects_omitted",
            "path": "$",
            "omittedCount": omitted,
            "permitted": "repair the reported defects, then resubmit for remaining validation"
        }));
    }
    if defects.is_empty() {
        return None;
    }
    Some(IpcError {
        code: error.code.clone(),
        message: format!(
            "Operation `{}` has invalid arguments. Repair every reported defect in one call and preserve all other valid arguments. All fields are direct top-level fields and columns/rows are native JSON arrays. Diagnostics: {}. Validation detail: {}",
            call.name,
            serde_json::Value::Array(defects),
            error.message
        ),
        retryable: error.retryable,
    })
}

fn append_row_length_defect(
    object: &serde_json::Map<String, serde_json::Value>,
    defects: &mut Vec<serde_json::Value>,
    maximum: usize,
) {
    let (Some(columns), Some(rows)) = (
        object.get("columns").and_then(serde_json::Value::as_array),
        object.get("rows").and_then(serde_json::Value::as_array),
    ) else {
        return;
    };
    let all = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            row.as_array().and_then(|cells| {
                (cells.len() != columns.len()).then(|| {
                    serde_json::json!({
                        "path": format!("$.rows[{index}]"),
                        "receivedCellCount": cells.len()
                    })
                })
            })
        })
        .collect::<Vec<_>>();
    if all.is_empty() {
        return;
    }
    let omitted = all.len().saturating_sub(maximum);
    defects.push(serde_json::json!({
        "code": "row_length_mismatch",
        "path": "$.rows",
        "received": all.into_iter().take(maximum).collect::<Vec<_>>(),
        "omittedCount": omitted,
        "permitted": { "cellCount": columns.len() }
    }));
}

fn append_delimited_array_defects(
    object: &serde_json::Map<String, serde_json::Value>,
    properties: &serde_json::Map<String, serde_json::Value>,
    defects: &mut Vec<serde_json::Value>,
) {
    for name in ["columns", "rows"] {
        let (Some(supplied), Some(schema)) = (object.get(name), properties.get(name)) else {
            continue;
        };
        let Some(items) = supplied.as_array() else {
            if let Some(defect) = schema_defect(schema, supplied, json_field_path("$", name)) {
                append_schema_repair_defect(defects, &defect, supplied);
            }
            continue;
        };
        if let Some(defect) = schema_container_defect(schema, supplied, json_field_path("$", name))
        {
            append_schema_repair_defect(defects, &defect, supplied);
        }
        let Some(item_schema) = schema.get("items") else {
            continue;
        };
        for (index, item) in items.iter().enumerate() {
            let item_path = format!("$.{name}[{index}]");
            if name == "rows"
                && let Some(cells) = item.as_array()
                && let Some(cell_schema) = item_schema.get("items")
            {
                if let Some(defect) = schema_container_defect(item_schema, item, item_path.clone())
                {
                    append_schema_repair_defect(defects, &defect, item);
                }
                for (cell_index, cell) in cells.iter().enumerate() {
                    if let Some(defect) =
                        schema_defect(cell_schema, cell, format!("{item_path}[{cell_index}]"))
                    {
                        append_schema_repair_defect(defects, &defect, cell);
                    }
                }
            } else if let Some(defect) = schema_defect(item_schema, item, item_path) {
                append_schema_repair_defect(defects, &defect, item);
            }
        }
    }
}

fn schema_container_defect(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: String,
) -> Option<SchemaDefect> {
    let mut container = schema.clone();
    container.as_object_mut()?.remove("items");
    schema_defect(&container, value, path)
}

fn append_schema_repair_defect(
    defects: &mut Vec<serde_json::Value>,
    defect: &SchemaDefect,
    received: &serde_json::Value,
) {
    defects.push(serde_json::json!({
        "code": "invalid_field",
        "path": defect.path,
        "received": bounded_received_value(received),
        "permitted": defect.expected
    }));
}

fn bounded_received_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) if text.len() > 160 => serde_json::json!({
            "type": "string",
            "length": text.len(),
            "prefix": text.chars().take(120).collect::<String>()
        }),
        serde_json::Value::Array(items) if items.len() > 16 => serde_json::json!({
            "type": "array",
            "length": items.len()
        }),
        serde_json::Value::Object(fields) if fields.len() > 16 => serde_json::json!({
            "type": "object",
            "fields": fields.keys().take(16).cloned().collect::<Vec<_>>()
        }),
        _ => value.clone(),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one deterministic schema traversal keeps path selection and supported keywords auditable"
)]
fn schema_defect(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: String,
) -> Option<SchemaDefect> {
    if let Some(variants) = schema.get("oneOf").and_then(serde_json::Value::as_array) {
        let defects = variants
            .iter()
            .map(|variant| schema_defect(variant, value, path.clone()))
            .collect::<Vec<_>>();
        if defects.iter().all(Option::is_some) {
            return defects.into_iter().flatten().next();
        }
    }
    if let Some(variants) = schema.get("anyOf").and_then(serde_json::Value::as_array) {
        let defects = variants
            .iter()
            .map(|variant| schema_defect(variant, value, path.clone()))
            .collect::<Vec<_>>();
        if defects.iter().all(Option::is_some) {
            return defects.into_iter().flatten().next();
        }
    }
    if let Some(expected) = schema.get("enum").and_then(serde_json::Value::as_array)
        && !expected.contains(value)
    {
        return Some(SchemaDefect {
            path,
            expected: format!("one of {}", serde_json::Value::Array(expected.clone())),
        });
    }
    if let Some(constant) = schema.get("const")
        && constant != value
    {
        return Some(SchemaDefect {
            path,
            expected: format!("the constant {constant}"),
        });
    }
    if let Some(expected_type) = schema.get("type")
        && !schema_type_matches(expected_type, value)
    {
        return Some(SchemaDefect {
            path,
            expected: format!("type {expected_type}"),
        });
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
            for name in required.iter().filter_map(serde_json::Value::as_str) {
                if !object.contains_key(name) {
                    return Some(SchemaDefect {
                        path: json_field_path(&path, name),
                        expected: "a required field".to_owned(),
                    });
                }
            }
        }
        if let Some(properties) = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for (name, item) in object {
                if let Some(property_schema) = properties.get(name) {
                    if let Some(defect) =
                        schema_defect(property_schema, item, json_field_path(&path, name))
                    {
                        return Some(defect);
                    }
                } else if schema.get("additionalProperties")
                    == Some(&serde_json::Value::Bool(false))
                {
                    let allowed = properties.keys().cloned().collect::<Vec<_>>().join(", ");
                    return Some(SchemaDefect {
                        path: json_field_path(&path, name),
                        expected: format!(
                            "no additional field; this closed object permits only: {allowed}"
                        ),
                    });
                }
            }
        }
    }
    if let Some(array) = value.as_array() {
        if let Some(minimum) = schema.get("minItems").and_then(serde_json::Value::as_u64)
            && array.len() < usize::try_from(minimum).unwrap_or(usize::MAX)
        {
            return Some(SchemaDefect {
                path,
                expected: format!("an array with at least {minimum} item(s)"),
            });
        }
        if let Some(maximum) = schema.get("maxItems").and_then(serde_json::Value::as_u64)
            && array.len() > usize::try_from(maximum).unwrap_or(usize::MAX)
        {
            return Some(SchemaDefect {
                path,
                expected: format!("an array with at most {maximum} item(s)"),
            });
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in array.iter().enumerate() {
                if let Some(defect) = schema_defect(item_schema, item, format!("{path}[{index}]")) {
                    return Some(defect);
                }
            }
        }
    }
    None
}

fn schema_type_matches(expected: &serde_json::Value, value: &serde_json::Value) -> bool {
    let matches = |name: &str| match name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "string" => value.is_string(),
        _ => false,
    };
    expected.as_str().is_some_and(matches)
        || expected.as_array().is_some_and(|types| {
            types
                .iter()
                .filter_map(serde_json::Value::as_str)
                .any(matches)
        })
}

fn json_field_path(parent: &str, field: &str) -> String {
    if field
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        format!("{parent}.{field}")
    } else {
        format!("{parent}[{}]", serde_json::json!(field))
    }
}
