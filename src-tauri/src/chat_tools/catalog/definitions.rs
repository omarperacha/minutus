use super::{
    DEFAULT_FIND_PATH_DEPTH, IpcError, MAX_PATH_REFERENCES, PromptToolDefinition,
    is_forecastable_chat_tool, tool_action_class, tool_metadata,
};

pub(super) fn validate_catalogue_metadata(
    definitions: &[PromptToolDefinition],
) -> Result<(), IpcError> {
    if definitions.iter().all(|definition| {
        tool_metadata(&definition.name).is_some()
            && (!is_forecastable_chat_tool(&definition.name)
                || tool_action_class(&definition.name).is_some())
    }) {
        Ok(())
    } else {
        Err(IpcError::internal())
    }
}

pub(super) fn write_delimited_table_tool_definition(
    aliases: &[serde_json::Value],
    available: &str,
) -> PromptToolDefinition {
    PromptToolDefinition {
        name: "filesystem.write_delimited_table".to_owned(),
        description: format!(
            "Create or replace exactly one CSV or TSV file. Canonical image-to-CSV and screenshot-to-table consumer: after filesystem.inspect_image, pass extracted values directly in top-level native JSON `columns` and `rows` arrays. Use it when you know the destination and complete table, rather than first narrating an OCR transcript. Do not use it for JSON, prose, source-code edits, partial cell updates, data analysis or validation beyond the user's requested outcome; use filesystem.write_text or filesystem.replace_text for those effects. Before submission, compare the complete requested source with the rows and columns, including trailing summary rows and partially populated rows. Follow explicit source notation and loaded output conventions: genuinely empty cells may be null, while explicit zero notation remains zero. Post-write verification proves the submitted bytes were written, not that the transcription is complete or correct. The host binds trusted evidence receipts, serializes cells deterministically, applies CSV/TSV escaping, writes atomically through the normal permission broker and verifies the post-write digest. Supply all six advertised fields directly at the top level. Never put `columns` or `rows` in a `changes` or `representation` wrapper and never encode either array as a JSON string. `workspace` selects an advertised writable workspace. `path` is the exact relative output path derived from user instructions or loaded convention evidence. `expectedSha256` is null only for a new file and otherwise the exact current lowercase digest. `delimiter` is `comma` for CSV or `tab` for TSV. `columns` is the non-empty header row. `rows` contains complete rows with exactly one string, number or null cell per column; null serializes as an empty cell. Example: {{\"workspace\":\"primary\",\"path\":\"outputs/example.csv\",\"expectedSha256\":null,\"delimiter\":\"comma\",\"columns\":[\"period\",\"amount\"],\"rows\":[[\"Example\",100]]}}. Available workspaces: {available}."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "columns": {
                    "type": "array", "minItems": 1, "maxItems": 128,
                    "description": "A native JSON array of header strings in output order; never a JSON-encoded string.",
                    "items": { "type": "string", "maxLength": 16384 }
                },
                "delimiter": {
                    "type": "string", "enum": ["comma", "tab"],
                    "description": "Use comma for CSV or tab for TSV."
                },
                "expectedSha256": {
                    "type": ["string", "null"],
                    "pattern": "^[0-9a-f]{64}$",
                    "description": "null only when creating a new file; the exact current digest when replacing one."
                },
                "path": {
                    "type": "string", "minLength": 1, "maxLength": 4096,
                    "description": "Exact destination path relative to the selected workspace."
                },
                "rows": {
                    "type": "array", "maxItems": 20000,
                    "description": "A native JSON array of complete row arrays; every row must contain exactly one string, number or null cell per column. Never a JSON-encoded string.",
                    "items": { "type": "array", "maxItems": 128, "items": { "type": ["string", "number", "null"] } }
                },
                "workspace": { "type": "string", "enum": aliases }
            },
            "required": ["workspace", "path", "expectedSha256", "delimiter", "columns", "rows"],
            "additionalProperties": false,
            "examples": [{
                "workspace": "primary",
                "path": "outputs/example.csv",
                "expectedSha256": null,
                "delimiter": "comma",
                "columns": ["period", "amount"],
                "rows": [["Example", 100]]
            }]
        }),
    }
}

pub(super) fn chat_create_directory_tool_definition(
    aliases: &[serde_json::Value],
    available: &str,
) -> PromptToolDefinition {
    PromptToolDefinition {
        name: "filesystem.create_directory".to_owned(),
        description: format!(
            "Create exactly one new directory beneath an existing parent. This is non-recursive: create parent directories in separate calls. Use it immediately before writing a file whose destination folder is missing. Available workspaces: {available}."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Strict relative path for the one directory to create."
                },
                "workspace": { "type": "string", "enum": aliases }
            },
            "required": ["workspace", "path"],
            "additionalProperties": false
        }),
    }
}

pub(super) fn forecastable_tool_names(definitions: &[PromptToolDefinition]) -> Vec<String> {
    definitions
        .iter()
        .filter(|definition| is_forecastable_chat_tool(&definition.name))
        .map(|definition| definition.name.clone())
        .collect()
}

pub(super) fn forecastable_tool_definitions(
    definitions: &[PromptToolDefinition],
) -> Vec<PromptToolDefinition> {
    definitions
        .iter()
        .filter(|definition| is_forecastable_chat_tool(&definition.name))
        .cloned()
        .collect()
}

pub(in crate::chat_tools) fn chat_find_paths_tool_definition(
    aliases: &[serde_json::Value],
    available: &str,
) -> PromptToolDefinition {
    PromptToolDefinition {
        name: "filesystem.find_paths".to_owned(),
        description: format!(
            "Find files or directories recursively by case-insensitive basename fragments without reading contents. Binary files such as images are included. Put up to eight independent filters in one call so related sources, examples and output manifests can be discovered together. Only nameContains is required per filter; path defaults to the workspace root, extensions to any, kind to any, and maxDepth to {DEFAULT_FIND_PATH_DEPTH}. Each result echoes its exact resolved query and reports matches plus traversal coverage, truncation, unsafe omissions and intentionally excluded directories. Up to {MAX_PATH_REFERENCES} returned matches include a conversation-scoped `pathRef`; pass that value verbatim as the `path` of a later exact read, stat, list, search or image inspection so the filename is not regenerated. Direct exact paths remain valid. complete is only a coverage flag: complete=true means the included scope was fully traversed within maxDepth. A returned relativePath is a validated usable observation even when complete=false. Do not repeat discovery merely because complete=false when a returned candidate already resolves the stage; refine only when the returned candidates are insufficient. Available workspaces: {available}."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "enum": aliases },
                "queries": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "default": ".",
                                "description": "Starting directory relative to the workspace. Use . for its root."
                            },
                            "nameContains": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 256,
                                "description": "Case-insensitive literal fragment matched against basenames."
                            },
                            "extensions": {
                                "type": "array",
                                "default": [],
                                "maxItems": 16,
                                "items": { "type": "string", "pattern": "^[A-Za-z0-9]{1,16}$" },
                                "description": "Optional case-insensitive extensions without leading dots."
                            },
                            "kind": {
                                "type": "string",
                                "enum": ["any", "file", "directory"],
                                "default": "any"
                            },
                            "maxDepth": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 32,
                                "default": DEFAULT_FIND_PATH_DEPTH
                            }
                        },
                        "required": ["nameContains"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["workspace", "queries"],
            "additionalProperties": false
        }),
    }
}

pub(in crate::chat_tools) fn chat_write_tool_definition(
    writable_aliases: &[serde_json::Value],
    available: &str,
) -> PromptToolDefinition {
    PromptToolDefinition {
        name: "filesystem.write_text".to_owned(),
        description: format!(
            "Directly replace complete UTF-8 file contents in a read-and-write project workspace. Use filesystem.replace_text for small edits. Existing files require their exact contentSha256 and createMissingParents=false. A new file requires expectedSha256=null; with createMissingParents=true the call must contain exactly one change and the broker may create only that target's validated missing ancestors. It reports createdParentDirectories, every file change, and one aggregate postconditionVerified. Discovery, digest, target, staging, or verification failure fails the call; changes and newly created parents are rolled back, while stale digests and existing targets are conflicts. A verified result is authoritative and undo removes newly created parents only when empty. Available workspaces: {available}."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "enum": writable_aliases },
                "changes": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "A file path relative to the selected workspace."
                            },
                            "expectedSha256": {
                                "type": ["string", "null"],
                                "pattern": "^[0-9a-f]{64}$"
                            },
                            "content": {
                                "type": "string",
                                "description": "The complete replacement UTF-8 file content."
                            },
                            "createMissingParents": {
                                "type": "boolean",
                                "description": "true only for one new-file change; creates only missing ancestors of that exact path."
                            }
                        },
                        "required": ["path", "expectedSha256", "content", "createMissingParents"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["workspace", "changes"],
            "additionalProperties": false
        }),
    }
}

pub(in crate::chat_tools) fn chat_replace_tool_definition(
    writable_aliases: &[serde_json::Value],
    available: &str,
) -> PromptToolDefinition {
    PromptToolDefinition {
        name: "filesystem.replace_text".to_owned(),
        description: format!(
            "Make small targeted edits to one existing UTF-8 file without reproducing its complete contents. Read the file first, pass its exact contentSha256, exact oldText, replacement newText and the required occurrence count. The broker verifies all matches, writes atomically, reopens the target for byte-and-digest verification, and retains a complete diff for review and undo. Prefer this over filesystem.write_text for localized edits. A successful verified result is authoritative: do not reread only to verify it; finish by answering the user. Available workspaces: {available}."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "enum": writable_aliases },
                "path": {
                    "type": "string",
                    "description": "An existing file path relative to the selected workspace."
                },
                "expectedSha256": {
                    "type": "string",
                    "pattern": "^[0-9a-f]{64}$"
                },
                "replacements": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 16,
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {
                                "type": "string",
                                "minLength": 1,
                                "description": "Exact current text, including whitespace where relevant."
                            },
                            "newText": { "type": "string" },
                            "expectedOccurrences": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 32
                            }
                        },
                        "required": ["oldText", "newText", "expectedOccurrences"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["workspace", "path", "expectedSha256", "replacements"],
            "additionalProperties": false
        }),
    }
}

pub(in crate::chat_tools) fn chat_scan_tool_definition(
    aliases: &[serde_json::Value],
    available: &str,
) -> PromptToolDefinition {
    PromptToolDefinition {
        name: "filesystem.scan".to_owned(),
        description: format!(
            "Scan bounded UTF-8 text files and return paths, byte sizes and line counts without returning contents. Use only for aggregate text-file size or line-count analysis. It omits binary files and is not filename search; use filesystem.find_paths for discovery. Common VCS, dependency, cache and build directories are excluded automatically. Available workspaces: {available}."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "enum": aliases },
                "path": {
                    "type": "string",
                    "description": "A directory relative to the selected workspace. Use . for its root."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to inspect safe descendant directories."
                },
                "minimumLineCount": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Return only files with at least this many lines."
                },
                "excludeExtensions": {
                    "type": "array",
                    "maxItems": 32,
                    "items": { "type": "string", "pattern": "^[A-Za-z0-9]{1,16}$" },
                    "description": "Case-insensitive extensions to exclude, without leading dots."
                }
            },
            "required": [
                "workspace",
                "path",
                "recursive",
                "minimumLineCount",
                "excludeExtensions"
            ],
            "additionalProperties": false
        }),
    }
}
