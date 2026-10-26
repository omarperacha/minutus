//! Closed artifact IR validation and deterministic serialization.

use std::collections::{BTreeMap, BTreeSet};

use ipc_types::{Conversation, IpcError, MessagePart, ToolOutcome, ToolResultPart};
use model_runtime::ParsedToolCall;

use super::validation::required_string_argument;

const MAX_EVIDENCE_BINDINGS: usize = 32;
const MAX_COLUMNS: usize = 128;
const MAX_ROWS: usize = 20_000;
const MAX_CELLS: usize = 200_000;
const MAX_TEXT_BYTES: usize = 1_048_576;
const MAX_JSON_DEPTH: usize = 32;
const MAX_JSON_NODES: usize = 100_000;

/// Canonicalizes a bounded JSON-encoded composite only where this closed tool
/// contract unambiguously requires an object. Unknown fields and scalar
/// coercions remain invalid.
pub(crate) fn normalize_arguments(call: &mut ParsedToolCall) -> bool {
    let Some(current) = call.arguments.get("representation") else {
        return false;
    };
    let (mut value, mut changed) = match current {
        serde_json::Value::String(encoded) if encoded.len() <= MAX_TEXT_BYTES => {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(encoded) else {
                return false;
            };
            if !value.is_object() {
                return false;
            }
            (value, true)
        }
        serde_json::Value::Object(_) => (current.clone(), false),
        _ => return false,
    };
    if let Some(fields) = value.as_object_mut()
        && fields.get("kind").and_then(serde_json::Value::as_str) == Some("delimited_table")
        && !fields.contains_key("delimiter")
    {
        fields.insert("delimiter".to_owned(), serde_json::json!("comma"));
        changed = true;
    }
    if changed {
        call.arguments.insert("representation".to_owned(), value);
    }
    changed
}

pub(super) fn materialize(
    conversation: &Conversation,
    call: &ParsedToolCall,
) -> Result<ParsedToolCall, IpcError> {
    validate_evidence_bindings(conversation, call)?;
    let content = serialize(
        call.arguments
            .get("representation")
            .ok_or_else(invalid_arguments)?,
    )?;
    let expected_sha256 = call
        .arguments
        .get("expectedSha256")
        .ok_or_else(invalid_arguments)?;
    if !expected_sha256.is_null() && expected_sha256.as_str().is_none() {
        return Err(invalid_arguments());
    }
    Ok(ParsedToolCall {
        call_id: call.call_id.clone(),
        recipient: "filesystem.write_text".to_owned(),
        name: "filesystem.write_text".to_owned(),
        arguments: BTreeMap::from([
            (
                "changes".to_owned(),
                serde_json::json!([{
                    "path": required_string_argument(&call.arguments, "path")?,
                    "expectedSha256": expected_sha256,
                    "content": content,
                    "createMissingParents": expected_sha256.is_null()
                }]),
            ),
            (
                "workspace".to_owned(),
                serde_json::Value::String(
                    required_string_argument(&call.arguments, "workspace")?.to_owned(),
                ),
            ),
        ]),
    })
}

/// Materializes the least-surprise delimited-table contract advertised from
/// semantic execution v2. Evidence remains host-bound and is deliberately not
/// part of the model-visible schema.
pub(super) fn materialize_delimited_table(
    conversation: &Conversation,
    call: &ParsedToolCall,
) -> Result<ParsedToolCall, IpcError> {
    if call.arguments.contains_key("evidence") {
        validate_evidence_bindings(conversation, call)?;
    }
    let content = serialize(&serde_json::json!({
        "kind": "delimited_table",
        "delimiter": call.arguments.get("delimiter").ok_or_else(invalid_arguments)?,
        "columns": call.arguments.get("columns").ok_or_else(invalid_arguments)?,
        "rows": call.arguments.get("rows").ok_or_else(invalid_arguments)?,
    }))?;
    materialized_write_call(call, &content)
}

fn materialized_write_call(
    call: &ParsedToolCall,
    content: &str,
) -> Result<ParsedToolCall, IpcError> {
    let expected_sha256 = call
        .arguments
        .get("expectedSha256")
        .ok_or_else(invalid_arguments)?;
    if !expected_sha256.is_null() && expected_sha256.as_str().is_none() {
        return Err(invalid_arguments());
    }
    Ok(ParsedToolCall {
        call_id: call.call_id.clone(),
        recipient: "filesystem.write_text".to_owned(),
        name: "filesystem.write_text".to_owned(),
        arguments: BTreeMap::from([
            (
                "changes".to_owned(),
                serde_json::json!([{
                    "path": required_string_argument(&call.arguments, "path")?,
                    "expectedSha256": expected_sha256,
                    "content": content,
                    "createMissingParents": expected_sha256.is_null()
                }]),
            ),
            (
                "workspace".to_owned(),
                serde_json::Value::String(
                    required_string_argument(&call.arguments, "workspace")?.to_owned(),
                ),
            ),
        ]),
    })
}

pub(super) fn validate_arguments(
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<(), IpcError> {
    let mut diagnostics = Vec::<String>::new();
    if !evidence_shape_valid(arguments.get("evidence")) {
        diagnostics.push(
            "evidence must contain exact host-bound source and convention receipts".to_owned(),
        );
    }
    if arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|path| path.is_empty() || path.len() > 4_096)
    {
        diagnostics.push("path must be one bounded relative workspace path".to_owned());
    }
    if !expected_digest_valid(arguments.get("expectedSha256")) {
        diagnostics.push(
            "expectedSha256 must be null for creation or one lowercase 64-character digest"
                .to_owned(),
        );
    }
    match arguments.get("representation") {
        Some(value) if value.is_object() => {
            diagnostics.extend(representation_diagnostics(value));
        }
        Some(_) => diagnostics.push(
            "representation must be a JSON object, never encoded JSON text or a bare rows array"
                .to_owned(),
        ),
        None => diagnostics.push("representation is required".to_owned()),
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(IpcError::invalid_request(format!(
            "Artifact transformation has {} independently detectable contract issue(s): {}",
            diagnostics.len(),
            diagnostics.join(" | ")
        )))
    }
}

pub(super) fn validate_delimited_table_arguments(
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<(), IpcError> {
    let mut diagnostics = Vec::<String>::new();
    if arguments
        .get("evidence")
        .is_some_and(|evidence| !evidence_shape_valid(Some(evidence)))
    {
        diagnostics.push(
            "evidence, when host-bound, must contain exact source and convention receipts"
                .to_owned(),
        );
    }
    if arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|path| path.is_empty() || path.len() > 4_096)
    {
        diagnostics.push("path must be one bounded relative workspace path".to_owned());
    }
    if !expected_digest_valid(arguments.get("expectedSha256")) {
        diagnostics.push(
            "expectedSha256 must be null for creation or one lowercase 64-character digest"
                .to_owned(),
        );
    }
    let representation = serde_json::json!({
        "kind": "delimited_table",
        "delimiter": arguments.get("delimiter").unwrap_or(&serde_json::Value::Null),
        "columns": arguments.get("columns").unwrap_or(&serde_json::Value::Null),
        "rows": arguments.get("rows").unwrap_or(&serde_json::Value::Null),
    });
    diagnostics.extend(
        representation_diagnostics(&representation)
            .into_iter()
            .map(|diagnostic| diagnostic.replace("representation.", "")),
    );
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(IpcError::invalid_request(format!(
            "filesystem.write_delimited_table has {} independently detectable contract issue(s): {}",
            diagnostics.len(),
            diagnostics.join(" | ")
        )))
    }
}

fn representation_diagnostics(value: &serde_json::Value) -> Vec<String> {
    let Some(fields) = value.as_object() else {
        return vec!["representation must be a JSON object".to_owned()];
    };
    let Some(kind) = fields.get("kind").and_then(serde_json::Value::as_str) else {
        return vec![
            "representation.kind is required and must be delimited_table, json_document, or text_lines".to_owned(),
        ];
    };
    let (required, allowed) = match kind {
        "delimited_table" => (
            &["columns", "rows"][..],
            &["columns", "delimiter", "kind", "rows"][..],
        ),
        "json_document" => (&["pretty", "value"][..], &["kind", "pretty", "value"][..]),
        "text_lines" => (&["lines"][..], &["kind", "lines"][..]),
        _ => {
            return vec![
                "representation.kind must be delimited_table, json_document, or text_lines"
                    .to_owned(),
            ];
        }
    };
    let field_issues = closed_field_diagnostics(fields, required, allowed);
    if !field_issues.is_empty() {
        return field_issues;
    }
    match kind {
        "delimited_table" => {
            if fields
                .get("delimiter")
                .is_some_and(|value| !matches!(value.as_str(), Some("comma" | "tab")))
            {
                return vec![
                    "representation.delimiter must be comma or tab when supplied".to_owned(),
                ];
            }
            if serialize(value).is_err() {
                vec![
                    "representation.columns must be bounded strings and every representation.rows entry must contain exactly the same number of string, number, or null cells".to_owned(),
                ]
            } else {
                Vec::new()
            }
        }
        "json_document" => {
            if serialize(value).is_err() {
                vec![
                    "representation.pretty must be boolean and value must fit the JSON limits"
                        .to_owned(),
                ]
            } else {
                Vec::new()
            }
        }
        "text_lines" => {
            if serialize(value).is_err() {
                vec![
                    "representation.lines must be bounded strings without embedded line breaks"
                        .to_owned(),
                ]
            } else {
                Vec::new()
            }
        }
        _ => unreachable!(),
    }
}

fn closed_field_diagnostics(
    fields: &serde_json::Map<String, serde_json::Value>,
    required: &[&str],
    allowed: &[&str],
) -> Vec<String> {
    let missing = required
        .iter()
        .filter(|field| !fields.contains_key(**field))
        .copied()
        .collect::<Vec<_>>();
    let unexpected = fields
        .keys()
        .filter(|field| !allowed.contains(&field.as_str()))
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut issues = Vec::new();
    if !missing.is_empty() {
        issues.push(format!(
            "representation missing required field(s) for its declared kind: {}",
            missing.join(", ")
        ));
    }
    if !unexpected.is_empty() {
        issues.push(format!(
            "representation has unexpected field(s) for its declared kind: {}",
            unexpected.join(", ")
        ));
    }
    issues
}

fn evidence_shape_valid(value: Option<&serde_json::Value>) -> bool {
    let Some(bindings) = value
        .and_then(serde_json::Value::as_array)
        .filter(|bindings| (2..=MAX_EVIDENCE_BINDINGS).contains(&bindings.len()))
    else {
        return false;
    };
    let roles = bindings
        .iter()
        .filter_map(serde_json::Value::as_object)
        .filter(|binding| binding.len() == 2)
        .filter_map(|binding| {
            let call_id = binding.get("callId")?.as_str()?;
            ((1..=128).contains(&call_id.len())).then(|| binding.get("role")?.as_str())?
        })
        .collect::<BTreeSet<_>>();
    roles.contains("source") && roles.contains("convention")
}

fn expected_digest_valid(value: Option<&serde_json::Value>) -> bool {
    value.is_some_and(|value| {
        value.is_null()
            || value.as_str().is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    })
}

fn validate_evidence_bindings(
    conversation: &Conversation,
    call: &ParsedToolCall,
) -> Result<(), IpcError> {
    let bindings = call
        .arguments
        .get("evidence")
        .and_then(serde_json::Value::as_array)
        .filter(|items| (2..=MAX_EVIDENCE_BINDINGS).contains(&items.len()))
        .ok_or_else(invalid_arguments)?;
    let completed = completed_results(conversation);
    let mut roles = BTreeSet::new();
    for binding in bindings {
        let fields = binding.as_object().ok_or_else(invalid_arguments)?;
        if fields.len() != 2 {
            return Err(invalid_arguments());
        }
        let call_id = fields
            .get("callId")
            .and_then(serde_json::Value::as_str)
            .filter(|call_id| (1..=128).contains(&call_id.len()))
            .ok_or_else(invalid_arguments)?;
        let role = fields
            .get("role")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(invalid_arguments)?;
        let parts = completed.get(call_id).ok_or_else(|| {
            IpcError::invalid_request(
                "Artifact transformation requires completed evidence from this conversation",
            )
        })?;
        match role {
            "source" if !parts.is_empty() => {}
            "convention"
                if parts.iter().any(|part| {
                    matches!(
                        part,
                        ToolResultPart::Json { .. } | ToolResultPart::Text { .. }
                    )
                }) => {}
            "source" | "convention" => {
                return Err(IpcError::invalid_request(
                    "Artifact transformation evidence does not satisfy its declared role",
                ));
            }
            _ => return Err(invalid_arguments()),
        }
        roles.insert(role);
    }
    if !roles.contains("source") || !roles.contains("convention") {
        return Err(IpcError::invalid_request(
            "Artifact transformation requires source and convention evidence",
        ));
    }
    Ok(())
}

fn completed_results(conversation: &Conversation) -> BTreeMap<&str, &Vec<ToolResultPart>> {
    let requested = conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::ToolCall { request } => Some(request.call_id.0.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::ToolResult { result } => match &result.outcome {
                ToolOutcome::Completed { content }
                    if requested.contains(result.call_id.0.as_str()) =>
                {
                    Some((result.call_id.0.as_str(), content))
                }
                ToolOutcome::Completed { .. } | ToolOutcome::Failed { .. } => None,
            },
            _ => None,
        })
        .collect()
}

fn serialize(value: &serde_json::Value) -> Result<String, IpcError> {
    let fields = value.as_object().ok_or_else(invalid_arguments)?;
    let kind = fields
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid_arguments)?;
    let content = match kind {
        "delimited_table" => serialize_table(fields)?,
        "json_document" => serialize_json(fields)?,
        "text_lines" => serialize_lines(fields)?,
        _ => return Err(invalid_arguments()),
    };
    if content.len() > MAX_TEXT_BYTES {
        return Err(IpcError::payload_too_large(
            "The artifact representation exceeds the bounded output size",
        ));
    }
    Ok(content)
}

fn serialize_table(
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, IpcError> {
    let keys = fields.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if keys != BTreeSet::from(["columns", "delimiter", "kind", "rows"])
        && keys != BTreeSet::from(["columns", "kind", "rows"])
    {
        return Err(invalid_arguments());
    }
    let columns = string_array(fields.get("columns"), 1, MAX_COLUMNS)?;
    let rows = fields
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .filter(|rows| rows.len() <= MAX_ROWS)
        .ok_or_else(invalid_arguments)?;
    let delimiter = match fields.get("delimiter").and_then(serde_json::Value::as_str) {
        None | Some("comma") => ',',
        Some("tab") => '\t',
        _ => return Err(invalid_arguments()),
    };
    let mut output = String::new();
    append_record(&mut output, &columns, delimiter);
    let mut cells = columns.len();
    for row in rows {
        let values = cell_array(Some(row), columns.len(), columns.len())?;
        cells = cells.saturating_add(values.len());
        if cells > MAX_CELLS {
            return Err(IpcError::payload_too_large(
                "The artifact table exceeds the bounded cell count",
            ));
        }
        append_record(&mut output, &values, delimiter);
    }
    Ok(output)
}

fn cell_array(
    value: Option<&serde_json::Value>,
    minimum: usize,
    maximum: usize,
) -> Result<Vec<String>, IpcError> {
    value
        .and_then(serde_json::Value::as_array)
        .filter(|values| (minimum..=maximum).contains(&values.len()))
        .ok_or_else(invalid_arguments)?
        .iter()
        .map(|value| match value {
            serde_json::Value::String(value) if value.len() <= 16_384 => Ok(value.clone()),
            serde_json::Value::Number(value) => Ok(value.to_string()),
            serde_json::Value::Null => Ok(String::new()),
            _ => Err(invalid_arguments()),
        })
        .collect()
}

fn append_record<T: AsRef<str>>(output: &mut String, fields: &[T], delimiter: char) {
    for (index, field) in fields.iter().enumerate() {
        let field = field.as_ref();
        if index != 0 {
            output.push(delimiter);
        }
        if field.contains(delimiter) || field.contains(['"', '\n', '\r']) {
            output.push('"');
            output.push_str(&field.replace('"', "\"\""));
            output.push('"');
        } else {
            output.push_str(field);
        }
    }
    output.push('\n');
}

fn serialize_json(fields: &serde_json::Map<String, serde_json::Value>) -> Result<String, IpcError> {
    if fields.keys().map(String::as_str).collect::<BTreeSet<_>>()
        != BTreeSet::from(["kind", "pretty", "value"])
    {
        return Err(invalid_arguments());
    }
    let pretty = fields
        .get("pretty")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(invalid_arguments)?;
    let value = fields.get("value").ok_or_else(invalid_arguments)?;
    let mut nodes = 0;
    let value = canonical_json(value, 0, &mut nodes)?;
    let mut output = if pretty {
        serde_json::to_string_pretty(&value)
    } else {
        serde_json::to_string(&value)
    }
    .map_err(|_| IpcError::internal())?;
    output.push('\n');
    Ok(output)
}

fn canonical_json(
    value: &serde_json::Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<serde_json::Value, IpcError> {
    *nodes = nodes.saturating_add(1);
    if depth > MAX_JSON_DEPTH || *nodes > MAX_JSON_NODES {
        return Err(IpcError::payload_too_large(
            "The JSON artifact exceeds the bounded structure limits",
        ));
    }
    match value {
        serde_json::Value::Array(values) => Ok(serde_json::Value::Array(
            values
                .iter()
                .map(|value| canonical_json(value, depth.saturating_add(1), nodes))
                .collect::<Result<_, _>>()?,
        )),
        serde_json::Value::Object(fields) => {
            let ordered = fields
                .iter()
                .map(|(key, value)| {
                    Ok((
                        key.clone(),
                        canonical_json(value, depth.saturating_add(1), nodes)?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, IpcError>>()?;
            Ok(serde_json::Value::Object(ordered.into_iter().collect()))
        }
        value => Ok(value.clone()),
    }
}

fn serialize_lines(
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, IpcError> {
    if fields.keys().map(String::as_str).collect::<BTreeSet<_>>()
        != BTreeSet::from(["kind", "lines"])
    {
        return Err(invalid_arguments());
    }
    let lines = string_array(fields.get("lines"), 0, MAX_ROWS)?;
    if lines.iter().any(|line| line.contains(['\n', '\r'])) {
        return Err(invalid_arguments());
    }
    let mut output = lines.join("\n");
    output.push('\n');
    Ok(output)
}

fn string_array(
    value: Option<&serde_json::Value>,
    minimum: usize,
    maximum: usize,
) -> Result<Vec<&str>, IpcError> {
    value
        .and_then(serde_json::Value::as_array)
        .filter(|values| (minimum..=maximum).contains(&values.len()))
        .ok_or_else(invalid_arguments)?
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| value.len() <= 16_384)
                .ok_or_else(invalid_arguments)
        })
        .collect()
}

fn invalid_arguments() -> IpcError {
    IpcError::invalid_request("The model returned an invalid artifact representation")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc_types::{
        AssetId, ConversationId, Message, MessageId, MessageRole, ProjectId, ToolCallId, ToolResult,
    };

    #[test]
    fn canonicalizes_only_a_json_encoded_representation_object() {
        let mut call = ParsedToolCall {
            call_id: "transform".to_owned(),
            recipient: "orchestration.transform_artifact".to_owned(),
            name: "orchestration.transform_artifact".to_owned(),
            arguments: BTreeMap::from([(
                "representation".to_owned(),
                serde_json::Value::String(
                    serde_json::json!({"kind":"text_lines","lines":["value"]}).to_string(),
                ),
            )]),
        };
        assert!(normalize_arguments(&mut call));
        assert!(call.arguments["representation"].is_object());

        call.arguments.insert(
            "representation".to_owned(),
            serde_json::Value::String("[1,2,3]".to_owned()),
        );
        assert!(!normalize_arguments(&mut call));
        assert!(call.arguments["representation"].is_string());

        call.arguments.insert(
            "representation".to_owned(),
            serde_json::json!({
                "kind":"delimited_table", "columns":["value"], "rows":[[1]]
            }),
        );
        assert!(normalize_arguments(&mut call));
        assert_eq!(call.arguments["representation"]["delimiter"], "comma");
    }

    fn evidence_conversation() -> Conversation {
        Conversation {
            id: ConversationId("conversation".to_owned()),
            project_id: ProjectId("project".to_owned()),
            title: "artifact".to_owned(),
            messages: vec![
                Message {
                    id: MessageId("assistant-call".to_owned()),
                    conversation_id: ConversationId("conversation".to_owned()),
                    role: MessageRole::Assistant,
                    parts: vec![MessagePart::ToolCall {
                        request: ipc_types::ToolRequest {
                            call_id: ToolCallId("evidence-1".to_owned()),
                            name: ipc_types::ToolName::FilesystemCollectEvidence,
                            arguments: BTreeMap::new(),
                        },
                    }],
                    created_at: "2026-08-23T00:00:00Z".to_owned(),
                },
                Message {
                    id: MessageId("tool-result".to_owned()),
                    conversation_id: ConversationId("conversation".to_owned()),
                    role: MessageRole::Tool,
                    parts: vec![MessagePart::ToolResult {
                        result: ToolResult {
                            call_id: ToolCallId("evidence-1".to_owned()),
                            outcome: ToolOutcome::Completed {
                                content: vec![
                                    ToolResultPart::Json {
                                        value: ipc_types::JsonValue::Object(BTreeMap::new()),
                                    },
                                    ToolResultPart::Asset {
                                        asset_id: AssetId("asset-1".to_owned()),
                                    },
                                ],
                            },
                        },
                    }],
                    created_at: "2026-08-23T00:00:00Z".to_owned(),
                },
            ],
            created_at: "2026-08-23T00:00:00Z".to_owned(),
            updated_at: "2026-08-23T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn csv_serialization_is_deterministic_and_escaped() -> Result<(), IpcError> {
        let value = serde_json::json!({
            "kind": "delimited_table",
            "columns": ["month", "amount"],
            "rows": [["Jan, 2026", "12\"34"]]
        });
        assert_eq!(
            serialize(&value)?,
            "month,amount\n\"Jan, 2026\",\"12\"\"34\"\n"
        );
        Ok(())
    }

    #[test]
    fn table_cells_accept_numbers_and_null_without_model_side_stringification()
    -> Result<(), IpcError> {
        let value = serde_json::json!({
            "kind": "delimited_table",
            "delimiter": "comma",
            "columns": ["month", "amount", "note"],
            "rows": [["Jan", 123.5, null], ["Feb", 0, null], ["Summary", null, "pending"]]
        });
        assert_eq!(
            serialize(&value)?,
            "month,amount,note\nJan,123.5,\nFeb,0,\nSummary,,pending\n"
        );
        Ok(())
    }

    #[test]
    fn argument_validation_reports_all_independently_detectable_contract_issues() {
        let arguments = BTreeMap::from([
            ("evidence".to_owned(), serde_json::json!([])),
            (
                "expectedSha256".to_owned(),
                serde_json::json!("not-a-digest"),
            ),
            ("path".to_owned(), serde_json::json!(null)),
            ("representation".to_owned(), serde_json::json!("[[1,2]]")),
        ]);
        let error = validate_arguments(&arguments)
            .err()
            .unwrap_or_else(IpcError::internal);
        assert!(error.message.contains("4 independently detectable"));
        assert!(error.message.contains("host-bound source and convention"));
        assert!(error.message.contains("bounded relative workspace path"));
        assert!(error.message.contains("lowercase 64-character digest"));
        assert!(error.message.contains("never encoded JSON text"));
    }

    #[test]
    fn representation_validation_names_the_invalid_nested_contract() {
        let common = [
            (
                "evidence".to_owned(),
                serde_json::json!([
                    {"callId":"source","role":"source"},
                    {"callId":"convention","role":"convention"}
                ]),
            ),
            ("expectedSha256".to_owned(), serde_json::Value::Null),
            ("path".to_owned(), serde_json::json!("artifacts/table.csv")),
        ];
        let mut missing_kind = BTreeMap::from(common.clone());
        missing_kind.insert(
            "representation".to_owned(),
            serde_json::json!({"columns":["value"],"rows":[[1]]}),
        );
        let error = validate_arguments(&missing_kind)
            .err()
            .unwrap_or_else(IpcError::internal);
        assert!(error.message.contains("representation.kind is required"));

        let mut missing_rows = BTreeMap::from(common);
        missing_rows.insert(
            "representation".to_owned(),
            serde_json::json!({"kind":"delimited_table","columns":["value"]}),
        );
        let error = validate_arguments(&missing_rows)
            .err()
            .unwrap_or_else(IpcError::internal);
        assert!(error.message.contains("missing required field(s)"));
        assert!(error.message.contains("rows"));
    }

    #[test]
    fn dissimilar_json_and_text_representations_are_supported() -> Result<(), IpcError> {
        assert_eq!(
            serialize(
                &serde_json::json!({"kind":"json_document","pretty":false,"value":{"ok":true}})
            )?,
            "{\"ok\":true}\n"
        );
        assert_eq!(
            serialize(&serde_json::json!({"kind":"text_lines","lines":["alpha","beta"]}))?,
            "alpha\nbeta\n"
        );
        Ok(())
    }

    #[test]
    fn malformed_table_is_rejected_without_serialization() {
        let value = serde_json::json!({
            "kind": "delimited_table", "delimiter": "comma",
            "columns": ["a", "b"], "rows": [["only-one"]]
        });
        assert!(serialize(&value).is_err());
    }

    #[test]
    fn materialization_binds_completed_evidence_and_emits_a_normal_write() -> Result<(), IpcError> {
        let call = ParsedToolCall {
            call_id: "transform-1".to_owned(),
            recipient: "orchestration.transform_artifact".to_owned(),
            name: "orchestration.transform_artifact".to_owned(),
            arguments: BTreeMap::from([
                (
                    "evidence".to_owned(),
                    serde_json::json!([
                        {"callId":"evidence-1","role":"source"},
                        {"callId":"evidence-1","role":"convention"}
                    ]),
                ),
                ("expectedSha256".to_owned(), serde_json::Value::Null),
                ("path".to_owned(), serde_json::json!("outputs/data.csv")),
                (
                    "representation".to_owned(),
                    serde_json::json!({
                        "kind":"delimited_table", "delimiter":"comma",
                        "columns":["month","amount"], "rows":[["Jan","10"]]
                    }),
                ),
                ("workspace".to_owned(), serde_json::json!("project")),
            ]),
        };
        let write = materialize(&evidence_conversation(), &call)?;
        assert_eq!(write.name, "filesystem.write_text");
        assert_eq!(write.call_id, call.call_id);
        assert_eq!(
            write.arguments["changes"][0]["content"],
            "month,amount\nJan,10\n"
        );
        assert_eq!(write.arguments["changes"][0]["path"], "outputs/data.csv");
        assert_eq!(write.arguments["changes"][0]["createMissingParents"], true);

        let mut stale = call;
        stale.arguments.insert(
            "evidence".to_owned(),
            serde_json::json!([
                {"callId":"missing","role":"source"},
                {"callId":"missing","role":"convention"}
            ]),
        );
        assert!(materialize(&evidence_conversation(), &stale).is_err());
        Ok(())
    }

    #[test]
    fn delimited_table_contract_materializes_without_a_representation_union() -> Result<(), IpcError>
    {
        let call = ParsedToolCall {
            call_id: "table-1".to_owned(),
            recipient: "filesystem.write_delimited_table".to_owned(),
            name: "filesystem.write_delimited_table".to_owned(),
            arguments: BTreeMap::from([
                (
                    "columns".to_owned(),
                    serde_json::json!(["period", "amount"]),
                ),
                ("delimiter".to_owned(), serde_json::json!("comma")),
                (
                    "evidence".to_owned(),
                    serde_json::json!([
                        {"callId":"evidence-1","role":"source"},
                        {"callId":"evidence-1","role":"convention"}
                    ]),
                ),
                ("expectedSha256".to_owned(), serde_json::Value::Null),
                ("path".to_owned(), serde_json::json!("outputs/example.csv")),
                ("rows".to_owned(), serde_json::json!([["Example", 100]])),
                ("workspace".to_owned(), serde_json::json!("project")),
            ]),
        };
        validate_delimited_table_arguments(&call.arguments)?;
        let write = materialize_delimited_table(&evidence_conversation(), &call)?;
        assert_eq!(write.name, "filesystem.write_text");
        assert_eq!(
            write.arguments["changes"][0]["content"],
            "period,amount\nExample,100\n"
        );
        assert_eq!(write.arguments["changes"][0]["path"], "outputs/example.csv");

        let mut self_contained = call;
        self_contained.arguments.remove("evidence");
        validate_delimited_table_arguments(&self_contained.arguments)?;
        assert!(
            materialize_delimited_table(
                &Conversation {
                    messages: Vec::new(),
                    ..evidence_conversation()
                },
                &self_contained
            )
            .is_ok()
        );
        Ok(())
    }

    #[test]
    fn delimited_table_contract_does_not_semantically_normalize_aliases() {
        let arguments = BTreeMap::from([
            ("columns".to_owned(), serde_json::json!(["value"])),
            ("evidence".to_owned(), serde_json::json!([])),
            ("expectedSha256".to_owned(), serde_json::Value::Null),
            ("path".to_owned(), serde_json::json!("outputs/example.csv")),
            ("rows".to_owned(), serde_json::json!([[1]])),
            ("variant".to_owned(), serde_json::json!("comma")),
            ("workspace".to_owned(), serde_json::json!("project")),
        ]);
        let error = validate_delimited_table_arguments(&arguments)
            .err()
            .unwrap_or_else(IpcError::internal);
        assert!(error.message.contains("delimiter"));
    }
}
