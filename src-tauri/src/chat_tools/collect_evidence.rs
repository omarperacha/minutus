//! Bounded composition of filename discovery, text loading, and image inspection.

use std::{collections::BTreeMap, path::Path};

use app_core::AppCore;
use ipc_types::{
    AssetId, FilesystemEntryKind, FilesystemFindPathKind, FilesystemFindPathQuery,
    FilesystemFindPathQueryResult, FilesystemFindPathsRequest, IpcError, ProjectId, ValidateIpc,
    WorkspaceGrantId,
};
use model_runtime::PromptToolDefinition;

use super::{
    ChatToolWorkspace, DEFAULT_FIND_PATH_DEPTH, filesystem::inspect_chat_image_path,
    validation::find_path_query_from_object,
};

const MAX_EVIDENCE_ITEMS: usize = 8;
const MAX_TEXT_ITEMS: usize = 2;
const MAX_IMAGE_ITEMS: usize = 1;
const MAX_MATCHES_PER_ITEM: usize = 16;
const MAX_COLLECTED_TEXT_BYTES: u64 = 256 * 1024;
const MAX_EVIDENCE_ID_BYTES: usize = 48;
const MAX_EVIDENCE_REQUIREMENT_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvidenceClass {
    IdentityLocation,
    FormatStructure,
    SemanticContent,
}

impl EvidenceClass {
    const fn name(self) -> &'static str {
        match self {
            Self::IdentityLocation => "identity_location",
            Self::FormatStructure => "format_structure",
            Self::SemanticContent => "semantic_content",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvidenceOperation {
    FindMatches,
    LoadExactTextFormat,
    LoadExactTextSemantic,
    LoadExactImageFormat,
    LoadExactImageSemantic,
}

impl EvidenceOperation {
    const fn name(self) -> &'static str {
        match self {
            Self::FindMatches => "find_matches",
            Self::LoadExactTextFormat => "load_exact_text_format",
            Self::LoadExactTextSemantic => "load_exact_text_semantic",
            Self::LoadExactImageFormat => "load_exact_image_format",
            Self::LoadExactImageSemantic => "load_exact_image_semantic",
        }
    }

    const fn class(self) -> EvidenceClass {
        match self {
            Self::FindMatches => EvidenceClass::IdentityLocation,
            Self::LoadExactTextFormat | Self::LoadExactImageFormat => {
                EvidenceClass::FormatStructure
            }
            Self::LoadExactTextSemantic | Self::LoadExactImageSemantic => {
                EvidenceClass::SemanticContent
            }
        }
    }

    const fn loads_text(self) -> bool {
        matches!(
            self,
            Self::LoadExactTextFormat | Self::LoadExactTextSemantic
        )
    }

    const fn loads_image(self) -> bool {
        matches!(
            self,
            Self::LoadExactImageFormat | Self::LoadExactImageSemantic
        )
    }

    const fn loads_content(self) -> bool {
        self.loads_text() || self.loads_image()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EvidenceItem {
    id: String,
    class: EvidenceClass,
    requirement: String,
    query: FilesystemFindPathQuery,
    exact_path: Option<String>,
    operation: EvidenceOperation,
}

fn discovery_item_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_EVIDENCE_ID_BYTES,
                "pattern": "^[A-Za-z0-9_-]+$"
            },
            "requirement": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_EVIDENCE_REQUIREMENT_BYTES,
                "description": "Concrete outcome dependency this evidence resolves; do not use confidence-seeking language."
            },
            "nameContains": { "type": "string", "maxLength": 256 },
            "operation": { "type": "string", "enum": ["find_matches"] },
            "scope": {
                "type": "string",
                "description": "directory searches immediate children only; descendants honors maxDepth.",
                "enum": ["directory", "descendants"],
                "default": "descendants"
            },
            "path": { "type": "string", "default": "." },
            "extensions": {
                "type": "array",
                "maxItems": 16,
                "items": { "type": "string", "pattern": "^[A-Za-z0-9]{1,16}$" }
            },
            "kind": { "type": "string", "enum": ["any", "file", "directory"] },
            "maxDepth": {
                "type": "integer",
                "minimum": 1,
                "maximum": 32,
                "default": DEFAULT_FIND_PATH_DEPTH
            }
        },
        "required": ["id", "requirement", "operation"],
        "anyOf": [
            {
                "required": ["nameContains"],
                "properties": { "nameContains": { "minLength": 1 } }
            },
            {
                "required": ["extensions"],
                "properties": { "extensions": { "minItems": 1 } }
            }
        ],
        "additionalProperties": false
    })
}

fn exact_load_item_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_EVIDENCE_ID_BYTES,
                "pattern": "^[A-Za-z0-9_-]+$"
            },
            "requirement": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_EVIDENCE_REQUIREMENT_BYTES,
                "description": "Concrete stage dependency this file resolves."
            },
            "operation": {
                "type": "string",
                "enum": ["load_exact_text_format", "load_exact_text_semantic", "load_exact_image_format", "load_exact_image_semantic"]
            },
            "path": {
                "type": "string",
                "minLength": 1,
                "description": "Exact relative path of one already-known file."
            }
        },
        "required": ["id", "requirement", "operation", "path"],
        "additionalProperties": false
    })
}

pub(super) fn tool_definition(
    aliases: &[serde_json::Value],
    available: &str,
) -> PromptToolDefinition {
    PromptToolDefinition {
        name: "filesystem.collect_evidence".to_owned(),
        description: format!(
            "Collect several bounded workspace evidence items in one call and return {{ready, coverageComplete, items}}. The top-level object contains exactly workspace and items; path belongs inside each item, never beside items. There are two selector modes. Discovery example: {{\"workspace\":\"primary\",\"items\":[{{\"id\":\"source\",\"requirement\":\"Locate the source image\",\"operation\":\"find_matches\",\"path\":\".\",\"nameContains\":\"table\",\"extensions\":[\"png\"]}}]}}. For discovery, use operation=find_matches with a search path plus non-empty nameContains, extensions, or both. Discovery matches include conversation-scoped `pathRef` values for later exact-path tools; copy a returned reference verbatim instead of regenerating its filename. For an already-known file, use one load_exact_* operation with its exact relative path or returned pathRef; no nameContains, extensions, scope, kind or maxDepth is permitted. Exact-load example item: {{\"id\":\"source\",\"requirement\":\"Load the selected image\",\"operation\":\"load_exact_image_semantic\",\"path\":\"inputs/source.png\"}}. An exact-load item has only id, requirement, operation and path. Text/image format operations load structure; semantic operations load content. Use the weakest operation that resolves the stage requirement. The host never chooses one file from ambiguous discovery. Each returned relativePath is a usable observation even when broader search coverage is incomplete; choose a sufficient candidate rather than repeating discovery merely because coverageComplete=false. Load independent known inputs together. Paths must come from the user or returned workspace evidence. Do not repeat evidence already loaded. Limits: {MAX_EVIDENCE_ITEMS} items, {MAX_TEXT_ITEMS} text loads, {MAX_IMAGE_ITEMS} image load, {MAX_MATCHES_PER_ITEM} matches per discovery item, and {MAX_COLLECTED_TEXT_BYTES} total text bytes. Available workspaces: {available}."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "enum": aliases },
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_EVIDENCE_ITEMS,
                    "items": { "oneOf": [discovery_item_schema(), exact_load_item_schema()] }
                }
            },
            "required": ["workspace", "items"],
            "additionalProperties": false
        }),
    }
}

pub(super) fn validate_arguments(
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<(), IpcError> {
    let items = parse_items(arguments)?;
    FilesystemFindPathsRequest {
        project_id: ProjectId("collect-evidence-validation".to_owned()),
        grant_id: WorkspaceGrantId("collect-evidence-validation".to_owned()),
        queries: items.into_iter().map(|item| item.query).collect(),
    }
    .validate()
}

pub(super) fn requests_image(
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<bool, IpcError> {
    parse_items(arguments).map(|items| items.iter().any(|item| item.operation.loads_image()))
}

pub(super) fn execute(
    core: &AppCore,
    workspace: &ChatToolWorkspace,
    resolved_root: &Path,
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<(serde_json::Value, Option<AssetId>), IpcError> {
    let items = parse_items(arguments)?;
    let queries = items
        .iter()
        .map(|item| item.query.clone())
        .collect::<Vec<_>>();
    let mut discovery = core
        .filesystem_find_paths(&workspace.grant_id, resolved_root, &queries)
        .map_err(crate::error_mapping::app_core_error)?;
    for (item, result) in items.iter().zip(&mut discovery.results) {
        if let Some(exact_path) = &item.exact_path {
            result
                .matches
                .retain(|entry| entry.relative_path == *exact_path);
        }
        if result.matches.len() > MAX_MATCHES_PER_ITEM {
            result.matches.truncate(MAX_MATCHES_PER_ITEM);
            result.truncated = true;
            result.complete = false;
        }
    }

    let mut ready = true;
    let mut text_bytes = 0_u64;
    let mut item_values = Vec::with_capacity(items.len());
    for (item, result) in items.iter().zip(&discovery.results) {
        let status = evidence_status(item, result);
        ready &= status == "ready";
        if status == "ready" && item.operation.loads_text() {
            text_bytes = text_bytes.saturating_add(
                result
                    .matches
                    .first()
                    .and_then(|entry| entry.byte_size)
                    .unwrap_or(u64::MAX),
            );
        }
        item_values.push(serde_json::json!({
            "id": item.id,
            "evidenceClass": item.class.name(),
            "requirement": item.requirement,
            "operation": item.operation.name(),
            "selectedPath": if status == "ready" && item.operation.loads_content() {
                result.matches.first().map(|entry| entry.relative_path.as_str())
            } else {
                None
            },
            "candidateCount": result.matches.len(),
            "status": status,
            "discovery": result,
        }));
    }
    if text_bytes > MAX_COLLECTED_TEXT_BYTES {
        return Err(IpcError::payload_too_large(format!(
            "Collected text evidence exceeds the {MAX_COLLECTED_TEXT_BYTES}-byte limit"
        )));
    }
    record_evidence_telemetry(core, &items, &discovery.results, ready);
    if !ready {
        return Ok((
            serde_json::json!({ "ready": false, "coverageComplete": false, "items": item_values }),
            None,
        ));
    }

    for (index, item) in items.iter().enumerate() {
        if item.operation.loads_text() {
            let path = &discovery.results[index].matches[0].relative_path;
            let text = core
                .filesystem_read_text(&workspace.grant_id, resolved_root, path)
                .map_err(crate::error_mapping::app_core_error)?;
            item_values[index]["text"] =
                serde_json::to_value(text).map_err(|_| IpcError::internal())?;
        }
    }

    let image_index = items.iter().position(|item| item.operation.loads_image());
    let inspected = if let Some(index) = image_index {
        let path = &discovery.results[index].matches[0].relative_path;
        let (asset_id, metadata) = inspect_chat_image_path(core, workspace, path, resolved_root)?;
        item_values[index]["image"] = metadata;
        Some(asset_id)
    } else {
        None
    };

    Ok((
        serde_json::json!({ "ready": true, "coverageComplete": true, "items": item_values }),
        inspected,
    ))
}

fn evidence_status(item: &EvidenceItem, result: &FilesystemFindPathQueryResult) -> &'static str {
    if result.truncated {
        return "incomplete";
    }
    match item.operation {
        EvidenceOperation::FindMatches if result.matches.is_empty() => "missing",
        EvidenceOperation::FindMatches => "ready",
        _ => match result.matches.as_slice() {
            [entry] if entry.kind == FilesystemEntryKind::File => "ready",
            [] => "missing",
            [_] => "not_file",
            _ => "ambiguous",
        },
    }
}

fn parse_items(
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<Vec<EvidenceItem>, IpcError> {
    let values = arguments
        .get("items")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(invalid_arguments)?;
    if values.is_empty() || values.len() > MAX_EVIDENCE_ITEMS {
        return Err(invalid_arguments());
    }
    let mut ids = Vec::with_capacity(values.len());
    let mut text_count = 0_usize;
    let mut image_count = 0_usize;
    let mut items = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let object = value.as_object().ok_or_else(invalid_arguments)?;
        let id = object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| valid_id(id) && !ids.iter().any(|existing| existing == id))
            .ok_or_else(invalid_arguments)?
            .to_owned();
        ids.push(id.clone());
        let requirement = object
            .get("requirement")
            .and_then(serde_json::Value::as_str)
            .filter(|value| (1..=MAX_EVIDENCE_REQUIREMENT_BYTES).contains(&value.len()))
            .ok_or_else(|| invalid_requirement(index, &id))?
            .to_owned();
        let operation = match object.get("operation").and_then(serde_json::Value::as_str) {
            Some("find_matches") => EvidenceOperation::FindMatches,
            Some("load_exact_text_format") => {
                text_count = text_count.saturating_add(1);
                EvidenceOperation::LoadExactTextFormat
            }
            Some("load_exact_text_semantic") => {
                text_count = text_count.saturating_add(1);
                EvidenceOperation::LoadExactTextSemantic
            }
            Some("load_exact_image_format") => {
                image_count = image_count.saturating_add(1);
                EvidenceOperation::LoadExactImageFormat
            }
            Some("load_exact_image_semantic") => {
                image_count = image_count.saturating_add(1);
                EvidenceOperation::LoadExactImageSemantic
            }
            _ => return Err(invalid_operation(index)),
        };
        let class = operation.class();
        if text_count > MAX_TEXT_ITEMS || image_count > MAX_IMAGE_ITEMS {
            return Err(invalid_arguments());
        }
        let (mut query, exact_path) = if operation == EvidenceOperation::FindMatches {
            let scope = match object.get("scope").and_then(serde_json::Value::as_str) {
                None | Some("descendants") => None,
                Some("directory") => Some(1),
                _ => return Err(invalid_arguments()),
            };
            let mut query =
                find_path_query_from_object(object, &["id", "operation", "requirement", "scope"])?;
            if query.name_contains.is_empty() && query.extensions.is_empty() {
                return Err(invalid_discovery_selector(index, &id));
            }
            if let Some(max_depth) = scope {
                query.max_depth = max_depth;
            }
            (query, None)
        } else {
            if !object
                .keys()
                .all(|key| matches!(key.as_str(), "id" | "operation" | "requirement" | "path"))
            {
                return Err(invalid_exact_selector(index, &id));
            }
            let path = object
                .get("path")
                .and_then(serde_json::Value::as_str)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| invalid_exact_selector(index, &id))?
                .to_owned();
            (exact_path_query(&path)?, Some(path))
        };
        if operation.loads_content() {
            query.kind = FilesystemFindPathKind::File;
        }
        items.push(EvidenceItem {
            id,
            class,
            requirement,
            query,
            exact_path,
            operation,
        });
    }
    Ok(items)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_EVIDENCE_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn invalid_arguments() -> IpcError {
    IpcError::invalid_request("The model returned invalid evidence-collection arguments")
}

fn invalid_operation(index: usize) -> IpcError {
    IpcError::invalid_request(format!(
        "items[{index}].operation must be one of find_matches, load_exact_text_format, load_exact_text_semantic, load_exact_image_format, or load_exact_image_semantic; the operation fixes both evidence strength and behavior, the host never selects a representative, and separate evidenceClass, mode, load, or selection fields are not accepted"
    ))
}

fn invalid_requirement(index: usize, id: &str) -> IpcError {
    IpcError::invalid_request(format!(
        "items[{index}] (`{id}`) must declare one concrete requirement of at most {MAX_EVIDENCE_REQUIREMENT_BYTES} bytes"
    ))
}

fn record_evidence_telemetry(
    core: &AppCore,
    items: &[EvidenceItem],
    results: &[FilesystemFindPathQueryResult],
    ready: bool,
) {
    #[cfg(debug_assertions)]
    core.record_development_tool_trace(&evidence_telemetry_value(items, results, ready));
    #[cfg(not(debug_assertions))]
    let _ = (core, items, results, ready);
}

#[cfg(debug_assertions)]
fn evidence_telemetry_value(
    items: &[EvidenceItem],
    results: &[FilesystemFindPathQueryResult],
    ready: bool,
) -> serde_json::Value {
    let statuses = items
        .iter()
        .zip(results)
        .map(|(item, result)| evidence_status(item, result))
        .collect::<Vec<_>>();
    serde_json::json!({
        "event": "evidence_collection_evaluated",
        "ready": ready,
        "coverageComplete": ready,
        "itemCount": items.len(),
        "evidenceClasses": items.iter().map(|item| item.class.name()).collect::<Vec<_>>(),
        "operations": items.iter().map(|item| item.operation.name()).collect::<Vec<_>>(),
        "statuses": statuses,
        "truncatedItems": results.iter().filter(|result| result.truncated).count(),
        "ambiguousItems": statuses.iter().filter(|status| **status == "ambiguous").count(),
        "retryableItems": statuses.iter().filter(|status| **status != "ready").count()
    })
}

fn exact_path_query(path: &str) -> Result<FilesystemFindPathQuery, IpcError> {
    let path = Path::new(path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(invalid_arguments)?;
    let parent = path
        .parent()
        .and_then(Path::to_str)
        .filter(|parent| !parent.is_empty())
        .unwrap_or(".");
    Ok(FilesystemFindPathQuery {
        relative_path: parent.to_owned(),
        name_contains: name.to_owned(),
        extensions: Vec::new(),
        kind: FilesystemFindPathKind::File,
        max_depth: 1,
    })
}

fn invalid_discovery_selector(index: usize, id: &str) -> IpcError {
    IpcError::invalid_request(format!(
        "items[{index}] (`{id}`) uses find_matches and must include a non-empty nameContains or at least one extension"
    ))
}

fn invalid_exact_selector(index: usize, id: &str) -> IpcError {
    IpcError::invalid_request(format!(
        "items[{index}] (`{id}`) uses an exact-load operation and must contain only id, requirement, operation, and one exact file path"
    ))
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs};

    use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};
    use ipc_types::WorkspacePermissionLevel;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn evidence_operation_is_closed_and_legacy_cross_products_are_rejected() -> Result<(), IpcError>
    {
        let definition = tool_definition(&[serde_json::json!("primary")], "primary");
        let variants = &definition.parameters["properties"]["items"]["items"]["oneOf"];
        let discovery = &variants[0]["properties"];
        let exact = &variants[1]["properties"];
        assert!(discovery.get("evidenceClass").is_none());
        assert!(exact.get("mode").is_none());
        assert!(exact.get("load").is_none());
        assert!(exact.get("selection").is_none());
        assert_eq!(
            exact["operation"]["enum"],
            serde_json::json!([
                "load_exact_text_format",
                "load_exact_text_semantic",
                "load_exact_image_format",
                "load_exact_image_semantic"
            ])
        );

        let legacy = BTreeMap::from([
            (
                "items".to_owned(),
                serde_json::json!([{
                    "id": "csv_sample",
                    "extensions": ["csv"],
                    "evidenceClass": "semantic_content",
                    "requirement": "Compare column meanings",
                    "load": "representative",
                    "selection": "representative"
                }]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]);
        let error = match validate_arguments(&legacy) {
            Ok(()) => {
                return Err(IpcError::invalid_request(
                    "legacy independent load and selection fields were accepted",
                ));
            }
            Err(error) => error,
        };
        assert!(error.message.contains("items[0].operation"));
        assert!(error.message.contains("evidenceClass"));
        assert!(error.message.contains("never selects a representative"));
        assert!(error.message.contains("separate evidenceClass"));
        Ok(())
    }

    #[test]
    fn selector_schema_and_validation_reject_directory_only_items_with_actionable_feedback()
    -> Result<(), IpcError> {
        let definition = tool_definition(&[serde_json::json!("primary")], "primary");
        assert!(definition.description.contains("two selector modes"));
        assert!(definition.description.contains("exact relative path"));
        assert!(definition.description.contains("no nameContains"));
        assert!(definition.description.contains("returned relativePath"));
        assert!(definition.description.contains("Do not repeat"));
        let item_schema = &definition.parameters["properties"]["items"]["items"];
        assert_eq!(
            item_schema["oneOf"][0]["anyOf"],
            serde_json::json!([
                {
                    "required": ["nameContains"],
                    "properties": {"nameContains": {"minLength": 1}}
                },
                {
                    "required": ["extensions"],
                    "properties": {"extensions": {"minItems": 1}}
                }
            ])
        );

        let arguments = BTreeMap::from([
            (
                "items".to_owned(),
                serde_json::json!([
                    {"id":"all_csvs","extensions":["csv"],"requirement":"Locate reconstructed tables","operation":"find_matches"},
                    {"id":"monthly_dir","path":"monthly cashflow","scope":"directory","requirement":"Locate monthly source files","operation":"find_matches"}
                ]),
            ),
            ("workspace".to_owned(), serde_json::json!("primary")),
        ]);
        let Err(error) = validate_arguments(&arguments) else {
            return Err(IpcError::invalid_request(
                "directory-only item was accepted",
            ));
        };
        assert!(error.message.contains("items[1]"));
        assert!(error.message.contains("monthly_dir"));
        assert!(error.message.contains("nameContains"));
        assert!(error.message.contains("at least one extension"));
        Ok(())
    }

    #[test]
    fn collection_discovers_and_loads_text_in_one_bounded_operation() -> Result<(), Box<dyn Error>>
    {
        let parent = TempDir::new()?;
        let data = parent.path().join("data");
        let workspace_root = parent.path().join("workspace");
        fs::create_dir_all(workspace_root.join("outputs/reconstructed_tables"))?;
        fs::create_dir_all(workspace_root.join("monthly cashflow"))?;
        fs::write(
            workspace_root.join("outputs/reconstructed_tables/manifest.json"),
            br#"{"files":[{"file":"annual cashflow/annual cashflow.csv"}]}"#,
        )?;
        fs::write(
            workspace_root.join("monthly cashflow/monthly cashflow 1.png"),
            png()?,
        )?;
        let core = AppCore::initialize(data)?;
        let grant = core.grant_workspace(&workspace_root, vec![1], "2026-08-18T12:00:00Z")?;
        let workspace = ChatToolWorkspace {
            alias: "primary".to_owned(),
            display_name: "workspace".to_owned(),
            grant_id: grant.id,
            permission_level: WorkspacePermissionLevel::ReadOnly,
        };
        let arguments = BTreeMap::from([(
            "items".to_owned(),
            serde_json::json!([
                {"id": "source", "path": "monthly cashflow/monthly cashflow 1.png", "requirement": "Transcribe the source table", "operation": "load_exact_image_semantic"},
                {"id": "convention", "path": "outputs/reconstructed_tables/manifest.json", "requirement": "Derive output naming and storage", "operation": "load_exact_text_format"}
            ]),
        )]);

        let (value, asset_id) = execute(&core, &workspace, &workspace_root, &arguments)
            .map_err(|error| std::io::Error::other(error.message))?;
        assert_eq!(value["ready"], serde_json::json!(true));
        assert_eq!(value["items"][0]["evidenceClass"], "semantic_content");
        assert_eq!(value["items"][1]["evidenceClass"], "format_structure");
        assert!(
            value["items"]
                .as_array()
                .is_some_and(|items| items.iter().all(|item| {
                    item["selectedPath"].as_str().is_none_or(|path| {
                        !Path::new(path)
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
                    })
                }))
        );
        assert!(
            value["items"][1]["text"]["text"]
                .as_str()
                .is_some_and(|text| text.contains("annual cashflow"))
        );
        assert_eq!(value["items"][0]["image"]["status"], "image_ready");
        assert!(asset_id.is_some());
        Ok(())
    }

    #[test]
    fn discovery_reports_multiple_candidates_without_loading() -> Result<(), Box<dyn Error>> {
        let parent = TempDir::new()?;
        let data = parent.path().join("data");
        let workspace_root = parent.path().join("workspace");
        fs::create_dir_all(&workspace_root)?;
        fs::write(workspace_root.join("manifest-a.json"), b"a")?;
        fs::write(workspace_root.join("manifest-b.json"), b"b")?;
        let core = AppCore::initialize(data)?;
        let grant = core.grant_workspace(&workspace_root, vec![1], "2026-08-18T12:00:00Z")?;
        let workspace = ChatToolWorkspace {
            alias: "primary".to_owned(),
            display_name: "workspace".to_owned(),
            grant_id: grant.id,
            permission_level: WorkspacePermissionLevel::ReadOnly,
        };
        let arguments = BTreeMap::from([(
            "items".to_owned(),
            serde_json::json!([{"id": "manifest", "nameContains": "manifest", "requirement": "Locate the declared format", "operation": "find_matches"}]),
        )]);

        let (value, asset_id) = execute(&core, &workspace, &workspace_root, &arguments)
            .map_err(|error| std::io::Error::other(error.message))?;
        assert_eq!(value["ready"], serde_json::json!(true));
        assert_eq!(value["items"][0]["candidateCount"], 2);
        assert_eq!(value["items"][0]["status"], serde_json::json!("ready"));
        assert!(value["items"][0].get("text").is_none());
        assert!(asset_id.is_none());
        Ok(())
    }

    #[test]
    fn stronger_semantic_example_requires_discovery_then_exact_model_selection()
    -> Result<(), Box<dyn Error>> {
        let parent = TempDir::new()?;
        let data = parent.path().join("data");
        let workspace_root = parent.path().join("workspace");
        fs::create_dir_all(workspace_root.join("source"))?;
        fs::create_dir_all(workspace_root.join("examples/nested"))?;
        fs::write(workspace_root.join("source/input.png"), png()?)?;
        fs::write(
            workspace_root.join("manifest.json"),
            br#"{"style":"house"}"#,
        )?;
        fs::write(workspace_root.join("examples/a.md"), "example a")?;
        fs::write(workspace_root.join("examples/b.md"), "example b")?;
        fs::write(workspace_root.join("examples/nested/ignored.md"), "nested")?;
        let core = AppCore::initialize(data)?;
        let grant = core.grant_workspace(&workspace_root, vec![1], "2026-08-18T12:00:00Z")?;
        let workspace = ChatToolWorkspace {
            alias: "primary".to_owned(),
            display_name: "workspace".to_owned(),
            grant_id: grant.id,
            permission_level: WorkspacePermissionLevel::ReadOnly,
        };
        let discovery_arguments = BTreeMap::from([(
            "items".to_owned(),
            serde_json::json!([{
                "id":"examples",
                "path":"examples",
                "extensions":["md"],
                "scope":"directory",
                "requirement":"Locate terminology examples",
                "operation":"find_matches"
            }]),
        )]);
        let (discovery, asset_id) =
            execute(&core, &workspace, &workspace_root, &discovery_arguments)
                .map_err(|error| std::io::Error::other(error.message))?;
        assert_eq!(discovery["ready"], true);
        assert_eq!(discovery["items"][0]["candidateCount"], 2);
        assert!(discovery["items"][0].get("text").is_none());
        assert!(asset_id.is_none());

        let exact_arguments = BTreeMap::from([(
            "items".to_owned(),
            serde_json::json!([
                {"id":"source","path":"source/input.png","requirement":"Read source values","operation":"load_exact_image_semantic"},
                {"id":"convention","path":"manifest.json","requirement":"Read the declared house format","operation":"load_exact_text_format"},
                {"id":"example","path":"examples/a.md","requirement":"Match terminology required by the user","operation":"load_exact_text_semantic"}
            ]),
        )]);
        let (value, asset_id) = execute(&core, &workspace, &workspace_root, &exact_arguments)
            .map_err(|error| std::io::Error::other(error.message))?;
        assert_eq!(value["ready"], true);
        assert_eq!(value["coverageComplete"], true);
        assert_eq!(value["items"][2]["candidateCount"], 1);
        assert_eq!(value["items"][2]["selectedPath"], "examples/a.md");
        assert_eq!(value["items"][2]["text"]["text"], "example a");
        assert!(asset_id.is_some());
        Ok(())
    }

    #[test]
    fn closed_operation_derives_proportional_strength_and_loading_behavior() -> Result<(), IpcError>
    {
        let layout = BTreeMap::from([(
            "items".to_owned(),
            serde_json::json!([{
                "id":"layout",
                "path":"layout.png",
                "requirement":"Match the required document layout",
                "operation":"load_exact_image_format"
            }]),
        )]);
        validate_arguments(&layout)?;
        assert!(requests_image(&layout)?);
        let parsed = parse_items(&layout)?;
        assert_eq!(parsed[0].class, EvidenceClass::FormatStructure);
        assert_eq!(parsed[0].operation, EvidenceOperation::LoadExactImageFormat);
        Ok(())
    }

    #[test]
    fn telemetry_exposes_evidence_control_state_without_requirements_or_content() {
        let query = FilesystemFindPathQuery {
            relative_path: ".".to_owned(),
            name_contains: "private name".to_owned(),
            extensions: Vec::new(),
            kind: FilesystemFindPathKind::File,
            max_depth: DEFAULT_FIND_PATH_DEPTH,
        };
        let items = vec![EvidenceItem {
            id: "private-id".to_owned(),
            class: EvidenceClass::FormatStructure,
            requirement: "private requirement".to_owned(),
            query: query.clone(),
            exact_path: Some("private name".to_owned()),
            operation: EvidenceOperation::LoadExactTextFormat,
        }];
        let results = vec![FilesystemFindPathQueryResult {
            query,
            matches: Vec::new(),
            directories_scanned: 1,
            omitted_count: 0,
            excluded_count: 0,
            truncated: true,
            complete: false,
        }];

        let telemetry = evidence_telemetry_value(&items, &results, false);
        assert_eq!(telemetry["evidenceClasses"][0], "format_structure");
        assert_eq!(telemetry["operations"][0], "load_exact_text_format");
        assert_eq!(telemetry["statuses"][0], "incomplete");
        assert_eq!(telemetry["truncatedItems"], 1);
        assert_eq!(telemetry["retryableItems"], 1);
        let rendered = telemetry.to_string();
        assert!(!rendered.contains("private-id"));
        assert!(!rendered.contains("private requirement"));
        assert!(!rendered.contains("private name"));
    }

    fn png() -> Result<Vec<u8>, image::ImageError> {
        let mut encoded = Vec::new();
        PngEncoder::new(&mut encoded).write_image(
            &[0_u8; 8 * 8 * 4],
            8,
            8,
            ColorType::Rgba8.into(),
        )?;
        Ok(encoded)
    }
}
