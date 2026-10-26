//! Bounded validation for untrusted Bazaar JSON Schemas.

use std::collections::BTreeSet;

use regex::Regex;
use serde_json::Value;

const MAX_SCHEMA_BYTES: usize = 32 * 1024;
const MAX_SCHEMA_DEPTH: usize = 20;
const MAX_SCHEMA_NODES: usize = 1_024;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct SchemaError {
    pub(super) path: String,
    pub(super) message: String,
}

pub(super) fn validate_schema(schema: &Value) -> Result<(), SchemaError> {
    if serde_json::to_vec(schema).map_or(true, |encoded| encoded.len() > MAX_SCHEMA_BYTES) {
        return Err(error("$", "schema exceeds the 32 KiB limit"));
    }
    let mut nodes = 0;
    inspect_schema(schema, schema, "$", 0, &mut nodes)
}

pub(super) fn validate_instance(schema: &Value, value: &Value) -> Result<(), SchemaError> {
    validate_schema(schema)?;
    validate(schema, schema, value, "$", 0)
}

fn inspect_schema(
    root: &Value,
    schema: &Value,
    path: &str,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), SchemaError> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(error(path, "schema nesting exceeds the bounded limit"));
    }
    *nodes = nodes.saturating_add(1);
    if *nodes > MAX_SCHEMA_NODES {
        return Err(error(path, "schema contains too many nodes"));
    }
    let object = schema
        .as_object()
        .ok_or_else(|| error(path, "schema nodes must be objects"))?;
    for (key, value) in object {
        match key.as_str() {
            "$schema" | "title" | "description" | "default" | "examples" | "readOnly"
            | "writeOnly" | "deprecated" | "pattern" | "format" | "minimum" | "maximum"
            | "exclusiveMinimum" | "exclusiveMaximum" | "uniqueItems" | "type" | "const"
            | "enum" | "required" => {}
            "minLength" | "maxLength" | "minItems" | "maxItems" | "minProperties"
            | "maxProperties" => validate_count_constraint(value, path, key)?,
            "additionalProperties" => {
                if value.is_object() {
                    inspect_schema(
                        root,
                        value,
                        &format!("{path}.{key}"),
                        depth.saturating_add(1),
                        nodes,
                    )?;
                } else if !value.is_boolean() {
                    return Err(error(
                        path,
                        "additionalProperties must be boolean or schema",
                    ));
                }
            }
            "$id" => {
                let id = value
                    .as_str()
                    .ok_or_else(|| error(path, "$id must be a string"))?;
                if !id.starts_with('#') {
                    return Err(error(path, "external or relative $id is not allowed"));
                }
            }
            "$ref" => {
                let reference = value
                    .as_str()
                    .ok_or_else(|| error(path, "$ref must be a string"))?;
                resolve_local_reference(root, reference)
                    .ok_or_else(|| error(path, "$ref must resolve within this schema"))?;
            }
            "properties" | "$defs" => {
                let children = value
                    .as_object()
                    .ok_or_else(|| error(path, "schema property map must be an object"))?;
                for (name, child) in children {
                    inspect_schema(
                        root,
                        child,
                        &format!("{path}.{key}.{name}"),
                        depth.saturating_add(1),
                        nodes,
                    )?;
                }
            }
            "items" | "not" => inspect_schema(
                root,
                value,
                &format!("{path}.{key}"),
                depth.saturating_add(1),
                nodes,
            )?,
            "oneOf" | "anyOf" | "allOf" => {
                let variants = value
                    .as_array()
                    .ok_or_else(|| error(path, "schema alternatives must be an array"))?;
                if variants.is_empty() || variants.len() > 16 {
                    return Err(error(
                        path,
                        "schema alternatives must contain 1 to 16 entries",
                    ));
                }
                for (index, variant) in variants.iter().enumerate() {
                    inspect_schema(
                        root,
                        variant,
                        &format!("{path}.{key}[{index}]"),
                        depth.saturating_add(1),
                        nodes,
                    )?;
                }
            }
            _ => {
                return Err(error(
                    &format!("{path}.{key}"),
                    "unsupported JSON Schema keyword",
                ));
            }
        }
    }
    Ok(())
}

fn validate(
    root: &Value,
    schema: &Value,
    value: &Value,
    path: &str,
    depth: usize,
) -> Result<(), SchemaError> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(error(
            path,
            "instance nesting exceeds bounded validation limit",
        ));
    }
    let object = schema
        .as_object()
        .ok_or_else(|| error(path, "schema node is not an object"))?;
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let target = resolve_local_reference(root, reference)
            .ok_or_else(|| error(path, "local schema reference is unresolved"))?;
        return validate(root, target, value, path, depth.saturating_add(1));
    }
    if let Some(expected) = object.get("const")
        && expected != value
    {
        return Err(error(path, &format!("expected constant {expected}")));
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array)
        && !values.contains(value)
    {
        return Err(error(
            path,
            &format!("expected one of {}", Value::Array(values.clone())),
        ));
    }
    if let Some(expected) = object.get("type")
        && !type_matches(expected, value)
    {
        return Err(error(path, &format!("expected type {expected}")));
    }
    if let Some(variants) = object.get("allOf").and_then(Value::as_array) {
        for variant in variants {
            validate(root, variant, value, path, depth.saturating_add(1))?;
        }
    }
    if let Some(variants) = object.get("anyOf").and_then(Value::as_array)
        && !variants
            .iter()
            .any(|variant| validate(root, variant, value, path, depth.saturating_add(1)).is_ok())
    {
        return Err(error(path, "value matches no allowed schema alternative"));
    }
    if let Some(variants) = object.get("oneOf").and_then(Value::as_array) {
        let matching = variants
            .iter()
            .filter(|variant| validate(root, variant, value, path, depth.saturating_add(1)).is_ok())
            .count();
        if matching != 1 {
            return Err(error(
                path,
                "value must match exactly one schema alternative",
            ));
        }
    }
    if let Some(negated) = object.get("not")
        && validate(root, negated, value, path, depth.saturating_add(1)).is_ok()
    {
        return Err(error(path, "value matches a prohibited schema"));
    }
    if let Some(text) = value.as_str() {
        validate_string(object, text, path)?;
    }
    if let Some(number) = value.as_f64() {
        validate_number(object, number, path)?;
    }
    if let Some(values) = value.as_array() {
        validate_array(root, object, values, path, depth)?;
    }
    if let Some(fields) = value.as_object() {
        validate_object(root, object, fields, path, depth)?;
    }
    Ok(())
}

fn validate_number(
    schema: &serde_json::Map<String, Value>,
    value: f64,
    path: &str,
) -> Result<(), SchemaError> {
    if schema
        .get("minimum")
        .and_then(Value::as_f64)
        .is_some_and(|minimum| value < minimum)
    {
        return Err(error(path, "number is below minimum"));
    }
    if schema
        .get("maximum")
        .and_then(Value::as_f64)
        .is_some_and(|maximum| value > maximum)
    {
        return Err(error(path, "number is above maximum"));
    }
    if schema
        .get("exclusiveMinimum")
        .and_then(Value::as_f64)
        .is_some_and(|minimum| value <= minimum)
    {
        return Err(error(path, "number is not above exclusiveMinimum"));
    }
    if schema
        .get("exclusiveMaximum")
        .and_then(Value::as_f64)
        .is_some_and(|maximum| value >= maximum)
    {
        return Err(error(path, "number is not below exclusiveMaximum"));
    }
    Ok(())
}

// IPC-safe JSON preserves numeric values as f64. Integral encodings such as
// 5.0 retain the same constraint as 5 after discovery is restored from history.
fn validate_count_constraint(value: &Value, path: &str, key: &str) -> Result<(), SchemaError> {
    nonnegative_count(value).map(|_| ()).ok_or_else(|| {
        error(
            &format!("{path}.{key}"),
            "count must be a nonnegative integer within u64 bounds",
        )
    })
}

fn nonnegative_count(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let number = value.as_f64()?;
        if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
            return None;
        }
        if number == 0.0 {
            return Some(0);
        }
        number.to_string().parse().ok()
    })
}

fn validate_string(
    schema: &serde_json::Map<String, Value>,
    value: &str,
    path: &str,
) -> Result<(), SchemaError> {
    let characters = value.chars().count();
    if schema
        .get("minLength")
        .and_then(nonnegative_count)
        .is_some_and(|minimum| characters < usize::try_from(minimum).unwrap_or(usize::MAX))
    {
        return Err(error(path, "string is shorter than minLength"));
    }
    if schema
        .get("maxLength")
        .and_then(nonnegative_count)
        .is_some_and(|maximum| characters > usize::try_from(maximum).unwrap_or(0))
    {
        return Err(error(path, "string is longer than maxLength"));
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        let expression =
            Regex::new(pattern).map_err(|_| error(path, "schema pattern is invalid"))?;
        if !expression.is_match(value) {
            return Err(error(path, "string does not match the required pattern"));
        }
    }
    if schema.get("format").and_then(Value::as_str) == Some("email") {
        let Some((local, domain)) = value.split_once('@') else {
            return Err(error(path, "string is not a valid email address"));
        };
        if local.is_empty()
            || domain.is_empty()
            || domain.contains('@')
            || !domain.contains('.')
            || value.chars().any(char::is_whitespace)
        {
            return Err(error(path, "string is not a valid email address"));
        }
    }
    Ok(())
}

fn validate_array(
    root: &Value,
    schema: &serde_json::Map<String, Value>,
    values: &[Value],
    path: &str,
    depth: usize,
) -> Result<(), SchemaError> {
    if schema
        .get("minItems")
        .and_then(nonnegative_count)
        .is_some_and(|minimum| values.len() < usize::try_from(minimum).unwrap_or(usize::MAX))
    {
        return Err(error(path, "array has fewer than minItems entries"));
    }
    if schema
        .get("maxItems")
        .and_then(nonnegative_count)
        .is_some_and(|maximum| values.len() > usize::try_from(maximum).unwrap_or(0))
    {
        return Err(error(path, "array has more than maxItems entries"));
    }
    if schema.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
        let unique = values.iter().map(Value::to_string).collect::<BTreeSet<_>>();
        if unique.len() != values.len() {
            return Err(error(path, "array entries must be unique"));
        }
    }
    if let Some(items) = schema.get("items") {
        for (index, item) in values.iter().enumerate() {
            validate(
                root,
                items,
                item,
                &format!("{path}[{index}]"),
                depth.saturating_add(1),
            )?;
        }
    }
    Ok(())
}

fn validate_object(
    root: &Value,
    schema: &serde_json::Map<String, Value>,
    fields: &serde_json::Map<String, Value>,
    path: &str,
    depth: usize,
) -> Result<(), SchemaError> {
    if schema
        .get("minProperties")
        .and_then(nonnegative_count)
        .is_some_and(|minimum| fields.len() < usize::try_from(minimum).unwrap_or(usize::MAX))
    {
        return Err(error(path, "object has fewer than minProperties fields"));
    }
    if schema
        .get("maxProperties")
        .and_then(nonnegative_count)
        .is_some_and(|maximum| fields.len() > usize::try_from(maximum).unwrap_or(0))
    {
        return Err(error(path, "object has more than maxProperties fields"));
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !fields.contains_key(name) {
                return Err(error(&field_path(path, name), "required field is missing"));
            }
        }
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    for (name, value) in fields {
        if let Some(property) = properties.and_then(|values| values.get(name)) {
            validate(
                root,
                property,
                value,
                &field_path(path, name),
                depth.saturating_add(1),
            )?;
            continue;
        }
        match schema.get("additionalProperties") {
            Some(Value::Bool(false)) => {
                return Err(error(
                    &field_path(path, name),
                    "additional field is not allowed",
                ));
            }
            Some(additional) if additional.is_object() => validate(
                root,
                additional,
                value,
                &field_path(path, name),
                depth.saturating_add(1),
            )?,
            _ => {}
        }
    }
    Ok(())
}

fn resolve_local_reference<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    if pointer.is_empty() {
        Some(root)
    } else if pointer.starts_with('/') {
        root.pointer(pointer)
    } else {
        None
    }
}

fn type_matches(expected: &Value, value: &Value) -> bool {
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
        || expected
            .as_array()
            .is_some_and(|types| types.iter().filter_map(Value::as_str).any(matches))
}

fn field_path(parent: &str, field: &str) -> String {
    if field
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        format!("{parent}.{field}")
    } else {
        format!("{parent}[{field:?}]")
    }
}

fn error(path: &str, message: &str) -> SchemaError {
    SchemaError {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_schemas_preserve_string_array_and_object_bounds()
    -> Result<(), Box<dyn std::error::Error>> {
        for (schema, invalid) in [
            (
                serde_json::json!({"type":"string","minLength":1}),
                serde_json::json!(""),
            ),
            (
                serde_json::json!({"type":"string","maxLength":1}),
                serde_json::json!("AB"),
            ),
            (
                serde_json::json!({"type":"array","minItems":1}),
                serde_json::json!([]),
            ),
            (
                serde_json::json!({"type":"array","maxItems":1}),
                serde_json::json!([1, 2]),
            ),
            (
                serde_json::json!({"type":"object","minProperties":1}),
                serde_json::json!({}),
            ),
            (
                serde_json::json!({"type":"object","maxProperties":1}),
                serde_json::json!({"a":1,"b":2}),
            ),
        ] {
            assert!(validate_instance(&schema, &invalid).is_err());
            let retained = crate::chat_tools::filesystem::ipc_json_value(&schema)
                .map_err(|error| error.message)?;
            let retained = serde_json::to_value(retained)?;
            assert!(
                validate_instance(&retained, &invalid).is_err(),
                "retention weakened {schema}"
            );
        }
        Ok(())
    }

    #[test]
    fn count_constraints_require_supported_nonnegative_integers() {
        for keyword in [
            "minLength",
            "maxLength",
            "minItems",
            "maxItems",
            "minProperties",
            "maxProperties",
        ] {
            for limit in [
                serde_json::json!(-1),
                serde_json::json!(1.5),
                serde_json::json!("1"),
                serde_json::Value::Null,
            ] {
                let schema = serde_json::json!({keyword: limit});
                assert!(
                    validate_schema(&schema).is_err(),
                    "invalid count accepted: {schema}"
                );
            }
            assert!(validate_schema(&serde_json::json!({keyword: 1.0})).is_ok());
        }
    }

    #[test]
    fn validates_closed_nested_body_and_discriminated_union() -> Result<(), String> {
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["choice"],
            "properties": {
                "choice": {
                    "oneOf": [
                        {"type":"object","required":["type","value"],"properties":{"type":{"const":"none"},"value":{"const":""}},"additionalProperties":false},
                        {"type":"object","required":["type","value"],"properties":{"type":{"const":"initials"},"value":{"type":"string","minLength":1,"maxLength":5}},"additionalProperties":false}
                    ]
                }
            }
        });
        assert!(
            validate_instance(
                &schema,
                &serde_json::json!({"choice":{"type":"initials","value":"TST"}})
            )
            .is_ok()
        );
        let Err(error) = validate_instance(
            &schema,
            &serde_json::json!({"choice":{"type":"initials","value":"TOO-LONG"}}),
        ) else {
            return Err("invalid value was accepted".to_owned());
        };
        assert_eq!(error.path, "$.choice");
        Ok(())
    }

    #[test]
    fn rejects_external_references_and_unknown_assertions() {
        assert!(
            validate_schema(&serde_json::json!({"$ref":"https://example.com/schema"})).is_err()
        );
        assert!(validate_schema(&serde_json::json!({"contains":{"type":"string"}})).is_err());
    }
}
