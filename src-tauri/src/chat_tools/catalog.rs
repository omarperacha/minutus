//! Workspace capability projection and model-facing tool schemas.

use app_core::{
    AppCore, PROJECT_COMPUTATION_TOOL, PURE_COMPUTATION_TOOL, ToolCategoryIndexEntry,
    computation_tool_registry, web_search_tool_registry, web_tool_registry,
};
use ipc_types::{IpcError, ProjectId, WorkspacePermissionLevel};
use model_runtime::PromptToolDefinition;

use crate::error_mapping::app_core_error;

use super::{
    ChatToolWorkspace, DEFAULT_FIND_PATH_DEPTH, collect_evidence, forecast_contract,
    is_forecastable_chat_tool, path_references::MAX_PATH_REFERENCES, plan_contract,
    tool_search_definition,
};

mod definitions;

use definitions::{
    chat_create_directory_tool_definition, forecastable_tool_definitions, forecastable_tool_names,
    validate_catalogue_metadata, write_delimited_table_tool_definition,
};
pub(super) use definitions::{
    chat_find_paths_tool_definition, chat_replace_tool_definition, chat_scan_tool_definition,
    chat_write_tool_definition,
};

const MAX_TOOLS_PER_PROJECTED_CATEGORY: usize = 6;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ToolCategory {
    WorkspaceDiscovery,
    WorkspaceRead,
    WorkspaceWrite,
    Computation,
    Web,
    Payments,
    Contracts,
    Orchestration,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ToolActionClass {
    Read,
    Compute,
    WorkspaceWrite,
    ExternalWrite,
}

impl ToolCategory {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "workspace_discovery" => Some(Self::WorkspaceDiscovery),
            "workspace_read" => Some(Self::WorkspaceRead),
            "workspace_write" => Some(Self::WorkspaceWrite),
            "computation" => Some(Self::Computation),
            "web" => Some(Self::Web),
            "contracts" => Some(Self::Contracts),
            "payments" => Some(Self::Payments),
            "orchestration" => Some(Self::Orchestration),
            _ => None,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceDiscovery => "workspace_discovery",
            Self::WorkspaceRead => "workspace_read",
            Self::WorkspaceWrite => "workspace_write",
            Self::Computation => "computation",
            Self::Web => "web",
            Self::Payments => "payments",
            Self::Contracts => "contracts",
            Self::Orchestration => "orchestration",
        }
    }

    const fn responsibility(self) -> &'static str {
        match self {
            Self::WorkspaceDiscovery => {
                "Locate or identify workspace resources without loading their semantic content."
            }
            Self::WorkspaceRead => {
                "Load or inspect bounded workspace evidence without changing the workspace."
            }
            Self::WorkspaceWrite => {
                "Create or modify authorized workspace resources with broker verification."
            }
            Self::Computation => {
                "Run bounded typed computation without granting filesystem or network authority."
            }
            Self::Web => {
                "Search or retrieve external evidence through separately mediated network operations."
            }
            Self::Contracts => {
                "Observe public EVM code and unsigned calls; separately qualified profile-limited writes require explicit native approval."
            }
            Self::Payments => {
                "Request a trusted human-approved payment through a narrowly scoped host flow."
            }
            Self::Orchestration => {
                "Declare, revise, or explicitly advance semantic plans and, for legacy contracts only, advisory execution forecasts; these controls grant no task authority."
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ToolCatalogueMetadata {
    name: &'static str,
    category: ToolCategory,
    action_class: Option<ToolActionClass>,
    common: bool,
}

const TOOL_CATALOGUE_METADATA: &[ToolCatalogueMetadata] = &[
    ToolCatalogueMetadata {
        name: "contract.request_workflow",
        category: ToolCategory::Contracts,
        action_class: Some(ToolActionClass::ExternalWrite),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "contract.inspect",
        category: ToolCategory::Contracts,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "contract.read",
        category: ToolCategory::Contracts,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.list",
        category: ToolCategory::WorkspaceDiscovery,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.find_paths",
        category: ToolCategory::WorkspaceDiscovery,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.stat",
        category: ToolCategory::WorkspaceDiscovery,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.collect_evidence",
        category: ToolCategory::WorkspaceRead,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.read_text",
        category: ToolCategory::WorkspaceRead,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.inspect_image",
        category: ToolCategory::WorkspaceRead,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.search",
        category: ToolCategory::WorkspaceRead,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.scan",
        category: ToolCategory::WorkspaceRead,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.create_directory",
        category: ToolCategory::WorkspaceWrite,
        action_class: Some(ToolActionClass::WorkspaceWrite),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.write_text",
        category: ToolCategory::WorkspaceWrite,
        action_class: Some(ToolActionClass::WorkspaceWrite),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.replace_text",
        category: ToolCategory::WorkspaceWrite,
        action_class: Some(ToolActionClass::WorkspaceWrite),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "filesystem.write_delimited_table",
        category: ToolCategory::WorkspaceWrite,
        action_class: Some(ToolActionClass::WorkspaceWrite),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "orchestration.transform_artifact",
        category: ToolCategory::WorkspaceWrite,
        action_class: Some(ToolActionClass::WorkspaceWrite),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "computation.run",
        category: ToolCategory::Computation,
        action_class: Some(ToolActionClass::Compute),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "computation.run_project",
        category: ToolCategory::Computation,
        action_class: Some(ToolActionClass::Compute),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "web.retrieve",
        category: ToolCategory::Web,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "web.search",
        category: ToolCategory::Web,
        action_class: Some(ToolActionClass::Read),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "wallet.request_transfer",
        category: ToolCategory::Payments,
        action_class: Some(ToolActionClass::ExternalWrite),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "x402.request",
        category: ToolCategory::Payments,
        action_class: Some(ToolActionClass::ExternalWrite),
        common: false,
    },
    ToolCatalogueMetadata {
        name: "orchestration.submit_plan",
        category: ToolCategory::Orchestration,
        action_class: None,
        common: true,
    },
    ToolCatalogueMetadata {
        name: "orchestration.submit_forecast",
        category: ToolCategory::Orchestration,
        action_class: None,
        common: true,
    },
    ToolCatalogueMetadata {
        name: "orchestration.complete_stage",
        category: ToolCategory::Orchestration,
        action_class: None,
        common: true,
    },
    ToolCatalogueMetadata {
        name: "orchestration.recall",
        category: ToolCategory::Orchestration,
        action_class: None,
        common: true,
    },
    ToolCatalogueMetadata {
        name: "orchestration.search_tools",
        category: ToolCategory::Orchestration,
        action_class: None,
        common: true,
    },
    ToolCatalogueMetadata {
        name: "orchestration.refine_stage",
        category: ToolCategory::Orchestration,
        action_class: None,
        common: false,
    },
];

fn tool_metadata(name: &str) -> Option<ToolCatalogueMetadata> {
    TOOL_CATALOGUE_METADATA
        .iter()
        .find(|metadata| metadata.name == name)
        .copied()
}

pub(crate) fn tool_category(name: &str) -> Option<ToolCategory> {
    tool_metadata(name).map(|metadata| metadata.category)
}

pub(crate) fn tool_action_class(name: &str) -> Option<ToolActionClass> {
    tool_metadata(name).and_then(|metadata| metadata.action_class)
}

/// Whether one authoritative, non-partial result exhausts the operation and
/// therefore satisfies its advisory semantic stage regardless of success.
///
/// This is deliberately narrower than action class: most failed effects leave
/// their stage unfinished, while a user-bounded one-shot payment attempt must
/// proceed to reporting instead of inviting another call or plan repair.
pub(crate) fn terminal_result_completes_stage(
    name: &str,
    arguments: &std::collections::BTreeMap<String, serde_json::Value>,
    outcome: &ipc_types::ToolOutcome,
) -> bool {
    (matches!(
        name,
        "wallet.request_transfer" | "contract.request_workflow"
    ) || (name == "x402.request"
        && arguments
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|operation| matches!(operation, "purchase" | "purchase_url"))))
        && match outcome {
            ipc_types::ToolOutcome::Completed { .. } => true,
            ipc_types::ToolOutcome::Failed { error } => !error.retryable,
        }
}

pub(crate) fn tool_category_name(name: &str) -> Option<&'static str> {
    tool_category(name).map(ToolCategory::as_str)
}

pub(crate) fn common_tool(name: &str) -> bool {
    tool_metadata(name).is_some_and(|metadata| metadata.common)
}

/// Canonical consumers promoted beside their producer, never an authority rule.
pub(crate) fn tool_companions(name: &str) -> &'static [&'static str] {
    match name {
        "filesystem.inspect_image" | "filesystem.collect_evidence" => {
            &["filesystem.write_delimited_table"]
        }
        _ => &[],
    }
}

/// These operations may carry a complete artifact in their structured arguments.
pub(crate) fn tool_has_bulk_arguments(name: &str) -> bool {
    matches!(
        name,
        "filesystem.write_delimited_table"
            | "filesystem.write_text"
            | "filesystem.replace_text"
            | "orchestration.transform_artifact"
    )
}

/// Bounded intent aliases for discovery, never authorization or operation names.
pub(super) fn tool_search_aliases(name: &str) -> &'static [&'static str] {
    match name {
        "contract.request_workflow" => &[
            "qualified token transfer",
            "self allowance",
            "dependent token pull",
        ],
        "contract.inspect" => &["contract metadata", "runtime code", "ABI provenance"],
        "contract.read" => &[
            "read contract",
            "allowance",
            "balanceOf",
            "unsigned call",
            "eth_call",
        ],
        "wallet.request_transfer" => &[
            "send ETH",
            "send USDC",
            "wallet transfer",
            "crypto transfer",
            "send tokens",
        ],
        "filesystem.inspect_image" => &[
            "read image",
            "inspect image",
            "image input",
            "image-to-csv",
            "image-to-table",
            "screenshot-to-table",
            "image-to-file",
            "screenshot-to-file",
        ],
        "filesystem.write_delimited_table" => &[
            "create csv",
            "create tsv",
            "write csv",
            "write tsv",
            "image-to-csv",
            "image-to-table",
            "screenshot-to-table",
            "csv from image",
            "table from screenshot",
        ],
        "filesystem.write_text" => &[
            "create text file",
            "write text file",
            "image-to-file",
            "screenshot-to-file",
        ],
        "filesystem.replace_text" => &["edit text file", "modify text file"],
        _ => &[],
    }
}

/// Stable discovery preference among equally relevant intent matches. A lower
/// value promotes the canonical structured consumer before a source reader or
/// generic writer without changing the set of matched operations.
pub(super) fn tool_search_preference(name: &str) -> u8 {
    match name {
        "filesystem.write_delimited_table" => 0,
        "filesystem.write_text" | "filesystem.replace_text" => 1,
        "filesystem.inspect_image" => 2,
        _ => 3,
    }
}

pub(crate) fn tool_category_index(
    definitions: &[PromptToolDefinition],
) -> Vec<ToolCategoryIndexEntry> {
    let available = definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    [
        ToolCategory::WorkspaceDiscovery,
        ToolCategory::WorkspaceRead,
        ToolCategory::WorkspaceWrite,
        ToolCategory::Computation,
        ToolCategory::Web,
        ToolCategory::Payments,
        ToolCategory::Contracts,
        ToolCategory::Orchestration,
    ]
    .into_iter()
    .filter_map(|category| {
        let operations = TOOL_CATALOGUE_METADATA
            .iter()
            .filter(|metadata| metadata.category == category && available.contains(metadata.name))
            .map(|metadata| metadata.name.to_owned())
            .collect::<Vec<_>>();
        (!operations.is_empty()).then(|| {
            ToolCategoryIndexEntry::new(
                category.as_str().to_owned(),
                category.responsibility().to_owned(),
                operations,
            )
        })
    })
    .collect()
}

pub(crate) fn forecastable_category_names(definitions: &[PromptToolDefinition]) -> Vec<String> {
    definitions
        .iter()
        .filter(|definition| is_forecastable_chat_tool(&definition.name))
        .filter_map(|definition| tool_category_name(&definition.name))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub(crate) fn category_tool_names(
    definitions: &[PromptToolDefinition],
    category: ToolCategory,
) -> Vec<&str> {
    definitions
        .iter()
        .filter(|definition| tool_category(&definition.name) == Some(category))
        .take(MAX_TOOLS_PER_PROJECTED_CATEGORY)
        .map(|definition| definition.name.as_str())
        .collect()
}

pub(crate) fn project_tool_definitions(
    definitions: &[PromptToolDefinition],
    categories: &std::collections::BTreeSet<ToolCategory>,
    explicit_names: &std::collections::BTreeSet<String>,
) -> Vec<PromptToolDefinition> {
    let category_names = categories
        .iter()
        .flat_map(|category| category_tool_names(definitions, *category))
        .collect::<std::collections::BTreeSet<_>>();
    definitions
        .iter()
        .filter(|definition| {
            common_tool(&definition.name)
                || explicit_names.contains(&definition.name)
                || category_names.contains(definition.name.as_str())
        })
        .cloned()
        .collect()
}

pub(crate) fn chat_tool_workspaces(
    core: &AppCore,
    project_id: &ProjectId,
) -> Result<Vec<ChatToolWorkspace>, IpcError> {
    let mut additional = 0_usize;
    core.project_workspaces(project_id)
        .map_err(app_core_error)?
        .into_iter()
        .map(|workspace| {
            let alias = match workspace.role {
                ipc_types::ProjectWorkspaceRole::Primary => "primary".to_owned(),
                ipc_types::ProjectWorkspaceRole::Additional => {
                    additional = additional.saturating_add(1);
                    format!("additional-{additional}")
                }
            };
            Ok(ChatToolWorkspace {
                alias,
                display_name: workspace.display_name,
                grant_id: workspace.grant_id,
                permission_level: workspace.permission_level,
            })
        })
        .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "catalogue assembly keeps capability-dependent tool ordering visible in one place"
)]
pub(crate) fn chat_tool_definitions(
    workspaces: &[ChatToolWorkspace],
) -> Result<Vec<PromptToolDefinition>, IpcError> {
    let mut computation = computation_tool_registry()
        .map_err(|_| IpcError::internal())?
        .model_definitions()
        .into_iter()
        .filter(|definition| !workspaces.is_empty() || definition.name == PURE_COMPUTATION_TOOL)
        .map(|definition| PromptToolDefinition {
            name: definition.name,
            description: definition.description,
            parameters: definition.parameters,
        })
        .collect::<Vec<_>>();
    computation.extend(
        web_tool_registry()
            .map_err(|_| IpcError::internal())?
            .model_definitions()
            .into_iter()
            .map(|definition| PromptToolDefinition {
                name: definition.name,
                description: definition.description,
                parameters: definition.parameters,
            }),
    );
    computation.push(super::x402::tool_definition());
    computation.push(super::wallet_transfer::tool_definition());
    computation.extend(super::contracts::definitions());
    computation.extend(
        web_search_tool_registry()
            .map_err(|_| IpcError::internal())?
            .model_definitions()
            .into_iter()
            .map(|definition| PromptToolDefinition {
                name: definition.name,
                description: definition.description,
                parameters: definition.parameters,
            }),
    );
    if workspaces.is_empty() {
        let mut definitions = computation;
        let forecastable_definitions = forecastable_tool_definitions(&definitions);
        let forecastable_tools = forecastable_tool_names(&definitions);
        definitions.push(plan_contract::tool_definition(&forecastable_tools));
        definitions.push(plan_contract::stage_completion_tool_definition());
        definitions.push(tool_search_definition());
        definitions.push(super::recall::tool_definition());
        let forecastable_categories = forecastable_category_names(&definitions);
        definitions.push(forecast_contract::tool_definition(
            &forecastable_definitions,
            &forecastable_categories,
        ));
        validate_catalogue_metadata(&definitions)?;
        return Ok(definitions);
    }
    let aliases = workspaces
        .iter()
        .map(|workspace| serde_json::Value::String(workspace.alias.clone()))
        .collect::<Vec<_>>();
    let available = workspaces
        .iter()
        .map(|workspace| workspace.alias.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let schema = |search: bool| {
        let mut properties = serde_json::Map::from_iter([
            (
                "workspace".to_owned(),
                serde_json::json!({ "type": "string", "enum": aliases }),
            ),
            (
                "path".to_owned(),
                serde_json::json!({
                    "type": "string",
                    "description": "Either an exact path relative to the selected workspace (use . for its root) or a `pathRef` value returned by discovery in this conversation. Copy the opaque reference verbatim; do not reconstruct the filename."
                }),
            ),
        ]);
        let mut required = vec!["workspace", "path"];
        if search {
            properties.insert(
                "query".to_owned(),
                serde_json::json!({
                    "type": "string",
                    "description": "A non-empty case-sensitive literal string to find."
                }),
            );
            required.push("query");
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        })
    };
    let mut definitions = computation;
    definitions.extend([
        collect_evidence::tool_definition(&aliases, &available),
        chat_find_paths_tool_definition(&aliases, &available),
    ]);
    definitions.extend([
        (
            "filesystem.list",
            "List immediate children of one already-known directory. It does not recurse. After the initial listing, the host permits at most two exact directories grounded by aggregate evidence or the initial result; it rejects duplicates and descent into directories learned only from another targeted listing. Use filesystem.collect_evidence when unknown files must be located and loaded together. Use scan only for aggregate text-file analysis.",
            false,
        ),
        (
            "filesystem.stat",
            "Inspect safe metadata for one known file or directory. Use filesystem.collect_evidence to locate unknown paths and load required evidence together.",
            false,
        ),
        (
            "filesystem.read_text",
            "Read one already-known bounded UTF-8 text file. Use filesystem.collect_evidence when the path is unknown or related files and images should be gathered together.",
            false,
        ),
        (
            "filesystem.inspect_image",
            "Read one already-known static PNG, JPEG or WebP image. For image-to-CSV or screenshot-to-table work, inspect once, then call filesystem.write_delimited_table with top-level native JSON `columns` and `rows` arrays. This tool creates no artifact. For another text-file artifact, use filesystem.write_text. Use filesystem.collect_evidence when the image path is unknown or related text evidence is also required. The image is validated, normalized and added to the conversation for visual analysis. For transcription, reproduce legible source values exactly. Arithmetic is a diagnostic for a possible visual misread, not authority to silently correct the source: recheck only disputed cells once, then preserve a clear inconsistent source value and note the anomaly concisely. Prefer direct structured consumer arguments over narrated OCR.",
            false,
        ),
        (
            "filesystem.search",
            "Recursively search inside bounded UTF-8 text files for a literal string. It omits binary files and does not search filenames; use filesystem.collect_evidence for filename discovery and loading.",
            true,
        ),
    ]
    .into_iter()
    .map(|(name, description, search)| PromptToolDefinition {
        name: name.to_owned(),
        description: format!("{description} Available workspaces: {available}."),
        parameters: schema(search),
    })
    .collect::<Vec<_>>());
    definitions.push(chat_scan_tool_definition(&aliases, &available));
    let writable_aliases = workspaces
        .iter()
        .filter(|workspace| workspace.permission_level == WorkspacePermissionLevel::ReadAndWrite)
        .map(|workspace| serde_json::Value::String(workspace.alias.clone()))
        .collect::<Vec<_>>();
    if !writable_aliases.is_empty() {
        definitions.push(chat_create_directory_tool_definition(
            &writable_aliases,
            &available,
        ));
        definitions.push(chat_write_tool_definition(&writable_aliases, &available));
        definitions.push(chat_replace_tool_definition(&writable_aliases, &available));
        definitions.push(write_delimited_table_tool_definition(
            &writable_aliases,
            &available,
        ));
    }
    let forecastable_definitions = forecastable_tool_definitions(&definitions);
    let forecastable_tools = forecastable_tool_names(&definitions);
    definitions.push(plan_contract::tool_definition(&forecastable_tools));
    definitions.push(plan_contract::stage_completion_tool_definition());
    definitions.push(tool_search_definition());
    definitions.push(super::recall::tool_definition());
    let forecastable_categories = forecastable_category_names(&definitions);
    definitions.push(forecast_contract::tool_definition(
        &forecastable_definitions,
        &forecastable_categories,
    ));
    validate_catalogue_metadata(&definitions)?;
    debug_assert!(
        definitions
            .iter()
            .any(|tool| tool.name == PURE_COMPUTATION_TOOL)
    );
    debug_assert!(
        definitions
            .iter()
            .any(|tool| tool.name == PROJECT_COMPUTATION_TOOL)
    );
    Ok(definitions)
}

#[cfg(test)]
mod tests;
