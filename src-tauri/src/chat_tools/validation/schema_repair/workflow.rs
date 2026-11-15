//! Bounded schema diagnostics only: never repairs or dispatches a declaration.
use super::{IpcError, PromptToolDefinition, json_field_path, schema_type_matches};
use crate::chat_tools::contracts::WORKFLOW_V2_EXAMPLE;
use serde_json::{Value, json};

const MAX_DEFECTS: usize = 32;
const MAX_VISITS: usize = 512;
const MAX_DEPTH: usize = 16;
const MAX_DIAGNOSTIC_BYTES: usize = 6 * 1024;
const MAX_MESSAGE_BYTES: usize = 8 * 1024;

pub(super) fn repair_error(
    tool: &PromptToolDefinition,
    error: &IpcError,
    value: &Value,
) -> Option<IpcError> {
    if tool.parameters["properties"]["contractVersion"]["const"] != 2 {
        return None; // Preserve the separate older profile-tool contract.
    }
    let mut defects = Defects::default();
    defects.visit(&tool.parameters, value, "$".into(), 0);
    if defects.items.is_empty() && !defects.omitted {
        return None; // Semantic errors retain the original correction path.
    }
    let message = format!(
        "Operation `contract.request_workflow` has invalid declaration arguments. Repair all reported JSON paths together using the advertised version2 schema; preserve valid user intent and unrelated arguments. Diagnostics: {}. Additional locations not inspected: {}. Use native JSON arrays/objects, not quoted JSON; do not add contract.read or raw transaction fields. Generic schema-valid example (illustrative target/function must be replaced by actually inspected user scope): {}. This example grants no authority. The host has not rewritten or executed this call; a newly emitted call must pass ordinary validation and admission.",
        Value::Array(defects.items),
        defects.omitted,
        WORKFLOW_V2_EXAMPLE,
    );
    if message.len() > MAX_MESSAGE_BYTES {
        return None;
    }
    Some(IpcError {
        code: error.code.clone(),
        message,
        retryable: error.retryable,
    })
}

#[derive(Default)]
struct Defects {
    items: Vec<Value>,
    bytes: usize,
    visits: usize,
    omitted: bool,
}
impl Defects {
    fn stopped(&mut self) -> bool {
        if self.omitted || self.items.len() >= MAX_DEFECTS || self.visits >= MAX_VISITS {
            self.omitted = true;
            true
        } else {
            false
        }
    }
    fn push(&mut self, path: &str, expected: &str, received: &str) {
        self.push_value(path, &Value::String(bounded(expected, 192)), received);
    }
    fn push_value(&mut self, path: &str, expected: &Value, received: &str) {
        if self.stopped() {
            return;
        }
        let clipped_path = bounded(path, 192);
        let path_truncated = clipped_path != path;
        let mut item = json!({"path":clipped_path,"expected":expected,"received":received});
        if path_truncated {
            item["pathTruncated"] = Value::Bool(true);
        }
        let bytes = item.to_string().len();
        if self.bytes.saturating_add(bytes + 1) > MAX_DIAGNOSTIC_BYTES {
            self.omitted = true;
            return;
        }
        self.bytes += bytes + 1;
        self.items.push(item);
    }
    fn visit(&mut self, schema: &Value, value: &Value, path: String, depth: usize) {
        if self.stopped() {
            return;
        }
        self.visits += 1;
        if depth > MAX_DEPTH {
            self.omitted = true;
            return;
        }
        if let Some(variants) = schema.get("oneOf").and_then(Value::as_array) {
            if let Some(variant) = variants.iter().find(|variant| {
                variant
                    .get("type")
                    .is_some_and(|kind| schema_type_matches(kind, value))
            }) {
                self.visit(variant, value, path, depth + 1);
            } else {
                self.push(
                    &path,
                    "an ABI string, boolean or native JSON array",
                    kind(value),
                );
            }
            return;
        }
        if let Some(constant) = schema.get("const")
            && constant != value
        {
            self.push(&path, &format!("the constant {constant}"), kind(value));
        }
        if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
            && !allowed.contains(value)
        {
            self.push_value(&path, &json!({"oneOf":allowed}), kind(value));
        }
        if let Some(expected) = schema.get("type")
            && !schema_type_matches(expected, value)
        {
            self.push(&path, &format!("native JSON type {expected}"), kind(value));
            return;
        }
        if let Some(object) = value.as_object() {
            self.object(schema, object, &path, depth);
        } else if let Some(items) = value.as_array() {
            self.array(schema, items, &path, depth);
        } else if let Some(text) = value.as_str() {
            for (key, too_short) in [("minLength", true), ("maxLength", false)] {
                if let Some(bound) = schema.get(key).and_then(Value::as_u64) {
                    let length = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
                    if (too_short && length < bound) || (!too_short && length > bound) {
                        self.push(&path, &format!("string {key} {bound}"), "string");
                    }
                }
            }
        }
    }
    fn object(
        &mut self,
        schema: &Value,
        object: &serde_json::Map<String, Value>,
        path: &str,
        depth: usize,
    ) {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    self.push(&json_field_path(path, name), "a required field", "missing");
                }
            }
        }
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return;
        };
        let mut fields = object.iter().collect::<Vec<_>>();
        fields.sort_by(|left, right| left.0.cmp(right.0));
        for (name, supplied) in fields {
            if self.stopped() {
                break;
            }
            let next = json_field_path(path, name);
            if let Some(property) = properties.get(name) {
                self.visit(property, supplied, next, depth + 1);
            } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                self.push(
                    &next,
                    "no field here; remove this unrecognized field",
                    kind(supplied),
                );
            }
        }
    }
    fn array(&mut self, schema: &Value, items: &[Value], path: &str, depth: usize) {
        for (key, too_short) in [("minItems", true), ("maxItems", false)] {
            if let Some(bound) = schema.get(key).and_then(Value::as_u64) {
                let length = u64::try_from(items.len()).unwrap_or(u64::MAX);
                if (too_short && length < bound) || (!too_short && length > bound) {
                    self.push(path, &format!("native JSON array {key} {bound}"), "array");
                }
            }
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in items.iter().enumerate() {
                if self.stopped() {
                    break;
                }
                self.visit(item_schema, item, format!("{path}[{index}]"), depth + 1);
            }
        }
    }
}
fn bounded(value: &str, chars: usize) -> String {
    let mut characters = value.chars();
    let mut output = characters.by_ref().take(chars).collect::<String>();
    if characters.next().is_some() {
        output.push('…');
    }
    output
}
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests;
