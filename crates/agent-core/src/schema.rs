use std::{collections::BTreeSet, fmt};

use regex::Regex;
use serde_json::{Map, Value};

const MAX_SCHEMA_DEPTH: usize = 16;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;

const ALLOWED_KEYWORDS: &[&str] = &[
    "additionalProperties",
    "description",
    "enum",
    "items",
    "maximum",
    "maxItems",
    "maxLength",
    "minimum",
    "minItems",
    "minLength",
    "pattern",
    "properties",
    "required",
    "type",
];

/// A validated, closed JSON Schema subset suitable for model tool arguments.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StrictJsonSchema(Value);

impl StrictJsonSchema {
    pub(crate) fn compile(schema: Value) -> Result<Self, SchemaError> {
        let encoded = serde_json::to_vec(&schema)
            .map_err(|_| SchemaError::invalid("$", "schema is not valid JSON"))?;
        if encoded.len() > MAX_SCHEMA_BYTES {
            return Err(SchemaError::invalid("$", "schema exceeds the size limit"));
        }
        validate_schema_node(&schema, "$", 0)?;
        let root = schema
            .as_object()
            .ok_or_else(|| SchemaError::invalid("$", "root schema must be an object"))?;
        if !allows_type(root, "object") {
            return Err(SchemaError::invalid(
                "$",
                "root schema must declare object arguments",
            ));
        }
        if root.get("additionalProperties") != Some(&Value::Bool(false)) {
            return Err(SchemaError::invalid(
                "$",
                "root object must reject unknown arguments",
            ));
        }
        Ok(Self(schema))
    }

    pub(crate) fn as_value(&self) -> &Value {
        &self.0
    }

    pub(crate) fn validate(&self, instance: &Value) -> Result<(), SchemaError> {
        validate_instance(&self.0, instance, "$", 0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SchemaError {
    path: String,
    message: &'static str,
}

impl SchemaError {
    fn invalid(path: &str, message: &'static str) -> Self {
        Self {
            path: path.to_owned(),
            message,
        }
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) const fn message(&self) -> &'static str {
        self.message
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at {}", self.message, self.path)
    }
}

fn validate_schema_node(schema: &Value, path: &str, depth: usize) -> Result<(), SchemaError> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(SchemaError::invalid(path, "schema nesting is too deep"));
    }
    let object = schema
        .as_object()
        .ok_or_else(|| SchemaError::invalid(path, "schema node must be an object"))?;
    for key in object.keys() {
        if !ALLOWED_KEYWORDS.contains(&key.as_str()) {
            return Err(SchemaError::invalid(path, "unsupported schema keyword"));
        }
    }
    let types = schema_types(object, path)?;
    if types.is_empty() {
        return Err(SchemaError::invalid(path, "schema type is required"));
    }

    if let Some(description) = object.get("description")
        && description.as_str().is_none_or(|value| value.len() > 4_096)
    {
        return Err(SchemaError::invalid(path, "schema description is invalid"));
    }
    if let Some(values) = object.get("enum") {
        let values = values
            .as_array()
            .ok_or_else(|| SchemaError::invalid(path, "enum must be an array"))?;
        if values.is_empty() || values.len() > 256 {
            return Err(SchemaError::invalid(path, "enum size is invalid"));
        }
    }
    validate_non_negative_integer_keyword(object, "minLength", path)?;
    validate_non_negative_integer_keyword(object, "maxLength", path)?;
    validate_non_negative_integer_keyword(object, "minItems", path)?;
    validate_non_negative_integer_keyword(object, "maxItems", path)?;
    validate_number_keyword(object, "minimum", path)?;
    validate_number_keyword(object, "maximum", path)?;
    if let Some(pattern) = object.get("pattern") {
        let pattern = pattern
            .as_str()
            .ok_or_else(|| SchemaError::invalid(path, "pattern must be a string"))?;
        if pattern.len() > 1_024 || Regex::new(pattern).is_err() {
            return Err(SchemaError::invalid(path, "pattern is invalid"));
        }
    }

    if types.contains("object") {
        validate_object_schema(object, path, depth)?;
    } else if object.contains_key("properties")
        || object.contains_key("required")
        || object.contains_key("additionalProperties")
    {
        return Err(SchemaError::invalid(
            path,
            "object keyword used on non-object",
        ));
    }
    if types.contains("array") {
        let items = object
            .get("items")
            .ok_or_else(|| SchemaError::invalid(path, "array items schema is required"))?;
        validate_schema_node(items, &format!("{path}.items"), depth + 1)?;
    } else if object.contains_key("items") {
        return Err(SchemaError::invalid(path, "items used on non-array"));
    }
    Ok(())
}

fn validate_object_schema(
    object: &Map<String, Value>,
    path: &str,
    depth: usize,
) -> Result<(), SchemaError> {
    if object.get("additionalProperties") != Some(&Value::Bool(false)) {
        return Err(SchemaError::invalid(
            path,
            "object schemas must reject unknown properties",
        ));
    }
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| SchemaError::invalid(path, "object properties are required"))?;
    if properties.len() > 128 {
        return Err(SchemaError::invalid(path, "too many object properties"));
    }
    for (name, child) in properties {
        if name.is_empty() || name.len() > 128 {
            return Err(SchemaError::invalid(path, "property name is invalid"));
        }
        validate_schema_node(child, &format!("{path}.properties.{name}"), depth + 1)?;
    }
    let required = object
        .get("required")
        .and_then(Value::as_array)
        .ok_or_else(|| SchemaError::invalid(path, "required property list is missing"))?;
    let mut seen = BTreeSet::new();
    for required_name in required {
        let required_name = required_name
            .as_str()
            .ok_or_else(|| SchemaError::invalid(path, "required names must be strings"))?;
        if !properties.contains_key(required_name) || !seen.insert(required_name) {
            return Err(SchemaError::invalid(path, "required property is invalid"));
        }
    }
    Ok(())
}

fn schema_types<'a>(
    object: &'a Map<String, Value>,
    path: &str,
) -> Result<BTreeSet<&'a str>, SchemaError> {
    let raw = object
        .get("type")
        .ok_or_else(|| SchemaError::invalid(path, "schema type is required"))?;
    let mut types = BTreeSet::new();
    match raw {
        Value::String(value) => {
            types.insert(value.as_str());
        }
        Value::Array(values) if !values.is_empty() => {
            for value in values {
                let value = value
                    .as_str()
                    .ok_or_else(|| SchemaError::invalid(path, "schema types must be strings"))?;
                if !types.insert(value) {
                    return Err(SchemaError::invalid(path, "schema types must be unique"));
                }
            }
        }
        _ => return Err(SchemaError::invalid(path, "schema type is invalid")),
    }
    if types.iter().any(|value| {
        !matches!(
            *value,
            "array" | "boolean" | "integer" | "null" | "number" | "object" | "string"
        )
    }) {
        return Err(SchemaError::invalid(path, "schema type is unsupported"));
    }
    Ok(types)
}

fn allows_type(object: &Map<String, Value>, expected: &str) -> bool {
    object.get("type").is_some_and(|value| match value {
        Value::String(value) => value == expected,
        Value::Array(values) => values.iter().any(|value| value.as_str() == Some(expected)),
        _ => false,
    })
}

fn validate_non_negative_integer_keyword(
    object: &Map<String, Value>,
    keyword: &str,
    path: &str,
) -> Result<(), SchemaError> {
    if object
        .get(keyword)
        .is_some_and(|value| value.as_u64().is_none())
    {
        return Err(SchemaError::invalid(path, "size constraint is invalid"));
    }
    Ok(())
}

fn validate_number_keyword(
    object: &Map<String, Value>,
    keyword: &str,
    path: &str,
) -> Result<(), SchemaError> {
    if object
        .get(keyword)
        .is_some_and(|value| value.as_f64().is_none())
    {
        return Err(SchemaError::invalid(path, "numeric constraint is invalid"));
    }
    Ok(())
}

fn validate_instance(
    schema: &Value,
    instance: &Value,
    path: &str,
    depth: usize,
) -> Result<(), SchemaError> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(SchemaError::invalid(path, "argument nesting is too deep"));
    }
    let object = schema
        .as_object()
        .ok_or_else(|| SchemaError::invalid(path, "trusted schema is invalid"))?;
    let types = schema_types(object, path)?;
    if !types
        .iter()
        .any(|kind| instance_matches_type(instance, kind))
    {
        return Err(SchemaError::invalid(path, "argument has the wrong type"));
    }
    if let Some(allowed) = object.get("enum").and_then(Value::as_array)
        && !allowed.contains(instance)
    {
        return Err(SchemaError::invalid(
            path,
            "argument is outside the allowed values",
        ));
    }
    match instance {
        Value::Object(instance_object) => {
            validate_object_instance(object, instance_object, path, depth)
        }
        Value::Array(values) => validate_array_instance(object, values, path, depth),
        Value::String(value) => validate_string_instance(object, value, path),
        Value::Number(value) => validate_number_instance(object, value.as_f64(), path),
        Value::Null | Value::Bool(_) => Ok(()),
    }
}

fn validate_object_instance(
    schema: &Map<String, Value>,
    instance: &Map<String, Value>,
    path: &str,
    depth: usize,
) -> Result<(), SchemaError> {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| SchemaError::invalid(path, "trusted object schema is invalid"))?;
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !instance.contains_key(name) {
                return Err(SchemaError::invalid(path, "required argument is missing"));
            }
        }
    }
    for (name, value) in instance {
        let child = properties
            .get(name)
            .ok_or_else(|| SchemaError::invalid(path, "unknown argument"))?;
        validate_instance(child, value, &format!("{path}.{name}"), depth + 1)?;
    }
    Ok(())
}

fn validate_array_instance(
    schema: &Map<String, Value>,
    instance: &[Value],
    path: &str,
    depth: usize,
) -> Result<(), SchemaError> {
    let length = u64::try_from(instance.len()).unwrap_or(u64::MAX);
    if schema
        .get("minItems")
        .and_then(Value::as_u64)
        .is_some_and(|min| length < min)
        || schema
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|max| length > max)
    {
        return Err(SchemaError::invalid(
            path,
            "array size is outside the allowed range",
        ));
    }
    let items = schema
        .get("items")
        .ok_or_else(|| SchemaError::invalid(path, "trusted array schema is invalid"))?;
    for (index, value) in instance.iter().enumerate() {
        validate_instance(items, value, &format!("{path}[{index}]"), depth + 1)?;
    }
    Ok(())
}

fn validate_string_instance(
    schema: &Map<String, Value>,
    instance: &str,
    path: &str,
) -> Result<(), SchemaError> {
    let length = u64::try_from(instance.chars().count()).unwrap_or(u64::MAX);
    if schema
        .get("minLength")
        .and_then(Value::as_u64)
        .is_some_and(|min| length < min)
        || schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|max| length > max)
    {
        return Err(SchemaError::invalid(
            path,
            "string length is outside the allowed range",
        ));
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        let regex = Regex::new(pattern)
            .map_err(|_| SchemaError::invalid(path, "trusted string pattern is invalid"))?;
        if !regex.is_match(instance) {
            return Err(SchemaError::invalid(
                path,
                "string does not match the required pattern",
            ));
        }
    }
    Ok(())
}

fn validate_number_instance(
    schema: &Map<String, Value>,
    instance: Option<f64>,
    path: &str,
) -> Result<(), SchemaError> {
    let instance = instance.ok_or_else(|| SchemaError::invalid(path, "number is invalid"))?;
    if schema
        .get("minimum")
        .and_then(Value::as_f64)
        .is_some_and(|min| instance < min)
        || schema
            .get("maximum")
            .and_then(Value::as_f64)
            .is_some_and(|max| instance > max)
    {
        return Err(SchemaError::invalid(
            path,
            "number is outside the allowed range",
        ));
    }
    Ok(())
}

fn instance_matches_type(instance: &Value, expected: &str) -> bool {
    match expected {
        "array" => instance.is_array(),
        "boolean" => instance.is_boolean(),
        "integer" => instance
            .as_number()
            .is_some_and(|number| number.as_i64().is_some() || number.as_u64().is_some()),
        "null" => instance.is_null(),
        "number" => instance.is_number(),
        "object" => instance.is_object(),
        "string" => instance.is_string(),
        _ => false,
    }
}
