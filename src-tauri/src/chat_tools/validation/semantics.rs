use super::{
    BTreeMap, DEFAULT_FIND_PATH_DEPTH, FilesystemFindPathKind, FilesystemFindPathQuery,
    FilesystemFindPathsRequest, FilesystemInspectImageRequest, FilesystemListRequest,
    FilesystemReadTextRequest, FilesystemReplaceTextRequest, FilesystemScanRequest,
    FilesystemSearchRequest, FilesystemStatRequest, FilesystemTextReplacement,
    FilesystemWriteTextChange, FilesystemWriteTextRequest, IpcError, ParsedToolCall, ProjectId,
    ToolName, ValidateIpc, WorkspaceGrantId, artifact_transform, collect_evidence, computation,
    web, web_search,
};

#[expect(
    clippy::too_many_lines,
    reason = "the closed tool enum is semantically validated in one exhaustive match"
)]
pub(in crate::chat_tools) fn validate_chat_tool_semantics(
    name: &ToolName,
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<(), IpcError> {
    let project_id = ProjectId("chat-tool-validation".to_owned());
    let grant_id = WorkspaceGrantId("chat-tool-validation".to_owned());
    match name {
        ToolName::FilesystemList => FilesystemListRequest {
            project_id,
            grant_id,
            relative_path: required_string_argument(arguments, "path")?.to_owned(),
        }
        .validate()?,
        ToolName::FilesystemFindPaths => FilesystemFindPathsRequest {
            project_id,
            grant_id,
            queries: required_find_path_queries_argument(arguments)?,
        }
        .validate()?,
        ToolName::FilesystemCollectEvidence => {
            collect_evidence::validate_arguments(arguments)?;
        }
        ToolName::FilesystemStat => FilesystemStatRequest {
            project_id,
            grant_id,
            relative_path: required_string_argument(arguments, "path")?.to_owned(),
        }
        .validate()?,
        ToolName::FilesystemReadText => FilesystemReadTextRequest {
            project_id,
            grant_id,
            relative_path: required_string_argument(arguments, "path")?.to_owned(),
        }
        .validate()?,
        ToolName::FilesystemInspectImage => FilesystemInspectImageRequest {
            project_id,
            grant_id,
            relative_path: required_string_argument(arguments, "path")?.to_owned(),
        }
        .validate()?,
        ToolName::FilesystemSearch => FilesystemSearchRequest {
            project_id,
            grant_id,
            relative_path: required_string_argument(arguments, "path")?.to_owned(),
            query: required_string_argument(arguments, "query")?.to_owned(),
        }
        .validate()?,
        ToolName::FilesystemScan => FilesystemScanRequest {
            project_id,
            grant_id,
            relative_path: required_string_argument(arguments, "path")?.to_owned(),
            recursive: required_bool_argument(arguments, "recursive")?,
            minimum_line_count: required_usize_argument(arguments, "minimumLineCount")?,
            excluded_extensions: required_string_array_argument(arguments, "excludeExtensions")?,
        }
        .validate()?,
        ToolName::FilesystemCreateDirectory => {
            let path = required_string_argument(arguments, "path")?;
            if path.is_empty() || path.len() > 4_096 {
                return Err(IpcError::invalid_request(
                    "The model returned invalid tool arguments",
                ));
            }
        }
        ToolName::OrchestrationTransformArtifact => {
            artifact_transform::validate_arguments(arguments)?;
        }
        ToolName::FilesystemWriteDelimitedTable => {
            artifact_transform::validate_delimited_table_arguments(arguments)?;
        }
        ToolName::FilesystemWriteText => FilesystemWriteTextRequest {
            project_id,
            grant_id,
            changes: required_write_changes_argument(arguments)?,
        }
        .validate()?,
        ToolName::FilesystemReplaceText => FilesystemReplaceTextRequest {
            project_id,
            grant_id,
            relative_path: required_string_argument(arguments, "path")?.to_owned(),
            expected_sha256: required_string_argument(arguments, "expectedSha256")?.to_owned(),
            replacements: required_text_replacements_argument(arguments)?,
        }
        .validate()?,
        ToolName::ComputationRun | ToolName::ComputationRunProject => {
            let tool_name = if *name == ToolName::ComputationRun {
                "computation.run"
            } else {
                "computation.run_project"
            };
            computation::validate_call(&ParsedToolCall {
                call_id: "computation-validation".to_owned(),
                recipient: tool_name.to_owned(),
                name: tool_name.to_owned(),
                arguments: arguments.clone(),
            })?;
        }
        ToolName::WebRetrieve => {
            let call = ParsedToolCall {
                call_id: "validation-web".to_owned(),
                recipient: "web.retrieve".to_owned(),
                name: "web.retrieve".to_owned(),
                arguments: arguments.clone(),
            };
            web::validate_call(&call)?;
        }
        ToolName::WebSearch => {
            let call = ParsedToolCall {
                call_id: "validation-search".to_owned(),
                recipient: "web.search".to_owned(),
                name: "web.search".to_owned(),
                arguments: arguments.clone(),
            };
            web_search::validate_call(&call)?;
        }
        ToolName::X402Request
        | ToolName::OrchestrationSubmitPlan
        | ToolName::OrchestrationSubmitForecast
        | ToolName::OrchestrationCompleteStage
        | ToolName::OrchestrationSearchTools
        | ToolName::OrchestrationRefineStage => {}
        _ => unreachable!("tool name was restricted above"),
    }
    Ok(())
}

pub(in crate::chat_tools) fn required_find_path_queries_argument(
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<Vec<FilesystemFindPathQuery>, IpcError> {
    arguments
        .get("queries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))?
        .iter()
        .map(|value| {
            let object = value.as_object().ok_or_else(|| {
                IpcError::invalid_request("The model returned invalid tool arguments")
            })?;
            find_path_query_from_object(object, &[])
        })
        .collect()
}

pub(in crate::chat_tools) fn find_path_query_from_object(
    object: &serde_json::Map<String, serde_json::Value>,
    additional_allowed: &[&str],
) -> Result<FilesystemFindPathQuery, IpcError> {
    let allowed = ["extensions", "kind", "maxDepth", "nameContains", "path"];
    if !object
        .keys()
        .all(|key| allowed.contains(&key.as_str()) || additional_allowed.contains(&key.as_str()))
    {
        return Err(IpcError::invalid_request(
            "The model returned invalid tool arguments",
        ));
    }
    let kind = match object.get("kind").and_then(serde_json::Value::as_str) {
        None | Some("any") => FilesystemFindPathKind::Any,
        Some("file") => FilesystemFindPathKind::File,
        Some("directory") => FilesystemFindPathKind::Directory,
        _ => {
            return Err(IpcError::invalid_request(
                "The model returned invalid tool arguments",
            ));
        }
    };
    Ok(FilesystemFindPathQuery {
        relative_path: optional_string_argument(object, "path")?
            .unwrap_or(".")
            .to_owned(),
        name_contains: optional_string_argument(object, "nameContains")?
            .unwrap_or_default()
            .to_owned(),
        extensions: optional_string_array_argument(object, "extensions")?.unwrap_or_default(),
        kind,
        max_depth: optional_usize_argument(object, "maxDepth")?.unwrap_or(DEFAULT_FIND_PATH_DEPTH),
    })
}

fn optional_string_argument<'a>(
    arguments: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<Option<&'a str>, IpcError> {
    arguments
        .get(name)
        .map(|value| {
            value.as_str().ok_or_else(|| {
                IpcError::invalid_request("The model returned invalid tool arguments")
            })
        })
        .transpose()
}

fn optional_usize_argument(
    arguments: &serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<Option<usize>, IpcError> {
    arguments
        .get(name)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    IpcError::invalid_request("The model returned invalid tool arguments")
                })
        })
        .transpose()
}

fn optional_string_array_argument(
    arguments: &serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<Option<Vec<String>>, IpcError> {
    arguments
        .get(name)
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| {
                    IpcError::invalid_request("The model returned invalid tool arguments")
                })?
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_owned).ok_or_else(|| {
                        IpcError::invalid_request("The model returned invalid tool arguments")
                    })
                })
                .collect()
        })
        .transpose()
}

pub(in crate::chat_tools) fn required_text_replacements_argument(
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<Vec<FilesystemTextReplacement>, IpcError> {
    arguments
        .get("replacements")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))?
        .iter()
        .map(|value| {
            let object = value.as_object().ok_or_else(|| {
                IpcError::invalid_request("The model returned invalid tool arguments")
            })?;
            if object
                .keys()
                .map(String::as_str)
                .ne(["expectedOccurrences", "newText", "oldText"]
                    .iter()
                    .copied())
            {
                return Err(IpcError::invalid_request(
                    "The model returned invalid tool arguments",
                ));
            }
            Ok(FilesystemTextReplacement {
                old_text: object
                    .get("oldText")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        IpcError::invalid_request("The model returned invalid tool arguments")
                    })?
                    .to_owned(),
                new_text: object
                    .get("newText")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        IpcError::invalid_request("The model returned invalid tool arguments")
                    })?
                    .to_owned(),
                expected_occurrences: object
                    .get("expectedOccurrences")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| {
                        IpcError::invalid_request("The model returned invalid tool arguments")
                    })?,
            })
        })
        .collect()
}

pub(in crate::chat_tools) fn required_write_changes_argument(
    arguments: &BTreeMap<String, serde_json::Value>,
) -> Result<Vec<FilesystemWriteTextChange>, IpcError> {
    arguments
        .get("changes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))?
        .iter()
        .map(|value| {
            let object = value.as_object().ok_or_else(|| {
                IpcError::invalid_request("The model returned invalid tool arguments")
            })?;
            if object.keys().map(String::as_str).ne([
                "content",
                "createMissingParents",
                "expectedSha256",
                "path",
            ]
            .iter()
            .copied())
            {
                return Err(IpcError::invalid_request(
                    "The model returned invalid tool arguments",
                ));
            }
            let expected_sha256 = match object.get("expectedSha256") {
                Some(serde_json::Value::String(value)) => Some(value.clone()),
                Some(serde_json::Value::Null) => None,
                _ => {
                    return Err(IpcError::invalid_request(
                        "The model returned invalid tool arguments",
                    ));
                }
            };
            Ok(FilesystemWriteTextChange {
                relative_path: object
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        IpcError::invalid_request("The model returned invalid tool arguments")
                    })?
                    .to_owned(),
                expected_sha256,
                content: object
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        IpcError::invalid_request("The model returned invalid tool arguments")
                    })?
                    .to_owned(),
                create_missing_parents: object
                    .get("createMissingParents")
                    .and_then(serde_json::Value::as_bool)
                    .ok_or_else(|| {
                        IpcError::invalid_request("The model returned invalid tool arguments")
                    })?,
            })
        })
        .collect()
}

pub(in crate::chat_tools) fn required_string_argument<'a>(
    arguments: &'a BTreeMap<String, serde_json::Value>,
    name: &str,
) -> Result<&'a str, IpcError> {
    arguments
        .get(name)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))
}

pub(in crate::chat_tools) fn required_bool_argument(
    arguments: &BTreeMap<String, serde_json::Value>,
    name: &str,
) -> Result<bool, IpcError> {
    arguments
        .get(name)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))
}

pub(in crate::chat_tools) fn required_usize_argument(
    arguments: &BTreeMap<String, serde_json::Value>,
    name: &str,
) -> Result<usize, IpcError> {
    arguments
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))
}

pub(in crate::chat_tools) fn required_string_array_argument(
    arguments: &BTreeMap<String, serde_json::Value>,
    name: &str,
) -> Result<Vec<String>, IpcError> {
    arguments
        .get(name)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| IpcError::invalid_request("The model returned invalid tool arguments"))?
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                IpcError::invalid_request("The model returned invalid tool arguments")
            })
        })
        .collect()
}
