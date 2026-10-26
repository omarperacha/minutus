//! On-demand discovery of model-visible task-tool contracts.

use std::collections::BTreeSet;

use ipc_types::IpcError;
use model_runtime::{ParsedToolCall, PromptToolDefinition};

use super::{
    catalog::{tool_search_aliases, tool_search_preference},
    is_forecastable_chat_tool, tool_category_name,
};

pub(crate) const SEARCH_TOOLS_TOOL: &str = "orchestration.search_tools";
const MAX_SEARCH_QUERIES: usize = 4;
const MAX_SEARCH_RESULTS: usize = 8;
const MAX_QUERY_BYTES: usize = 96;
const MAX_SUMMARY_CHARS: usize = 240;

pub(crate) struct SearchToolsOutcome {
    pub(crate) value: serde_json::Value,
    pub(crate) loaded_names: Vec<String>,
}

pub(crate) fn tool_definition() -> PromptToolDefinition {
    PromptToolDefinition {
        name: SEARCH_TOOLS_TOOL.to_owned(),
        description: "Search the complete available task-tool catalogue when the promoted plan suggestions do not contain a suitable operation. This is a discovery control, not a permission request or plan revision. Search by exact operation name, namespace, category, or a short outcome-oriented capability phrase. Matching complete schemas become directly callable on the next round while the accepted plan and current stage remain unchanged. Intent queries such as image-to-CSV or screenshot-to-table promote the canonical structured consumer and the separate image reader; image-to-file promotes the generic text writer and image reader. Put all independent searches in one queries array. Example: {\"queries\":[\"image-to-CSV\"]}."
            .to_owned(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "queries": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_SEARCH_QUERIES,
                    "items": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MAX_QUERY_BYTES
                    },
                    "description": "One or more exact operation names, namespaces, categories, or concise capability phrases."
                }
            },
            "required": ["queries"],
            "additionalProperties": false,
            "examples": [{"queries": ["image-to-CSV"]}]
        }),
    }
}

pub(crate) fn search_tool_definitions(
    call: &ParsedToolCall,
    catalogue: &[PromptToolDefinition],
) -> Result<SearchToolsOutcome, IpcError> {
    let queries = call
        .arguments
        .get("queries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(invalid_search)?;
    if queries.is_empty() || queries.len() > MAX_SEARCH_QUERIES {
        return Err(invalid_search());
    }
    let queries = queries
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|query| !query.is_empty() && query.len() <= MAX_QUERY_BYTES)
                .map(str::to_ascii_lowercase)
                .ok_or_else(invalid_search)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut matches = catalogue
        .iter()
        .filter(|tool| is_forecastable_chat_tool(&tool.name))
        .filter_map(|tool| {
            let category = tool_category_name(&tool.name).unwrap_or("uncategorized");
            let rank = queries
                .iter()
                .filter_map(|query| match_rank(query, tool, category))
                .min()?;
            Some((rank, tool_search_preference(&tool.name), tool, category))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.name.cmp(&right.2.name))
    });

    let mut seen = BTreeSet::new();
    let selected = matches
        .into_iter()
        .filter(|(_, _, tool, _)| seen.insert(tool.name.clone()))
        .take(MAX_SEARCH_RESULTS)
        .collect::<Vec<_>>();
    let loaded_names = selected
        .iter()
        .map(|(_, _, tool, _)| tool.name.clone())
        .collect::<Vec<_>>();
    let results = selected
        .into_iter()
        .map(|(_, _, tool, category)| {
            serde_json::json!({
                "name": tool.name,
                "category": category,
                "summary": tool.description.chars().take(MAX_SUMMARY_CHARS).collect::<String>(),
                "schemaLoadedForNextRound": true
            })
        })
        .collect::<Vec<_>>();
    Ok(SearchToolsOutcome {
        value: serde_json::json!({
            "queries": queries,
            "matches": results,
            "loaded": loaded_names,
            "planRevisionRequired": false,
            "message": if results.is_empty() {
                "No matching available task tool was found. Try one exact namespace, category, or operation name."
            } else {
                "Matching task schemas are directly callable on the next round; continue the current stage without revising the plan unless its semantics changed."
            }
        }),
        loaded_names,
    })
}

fn match_rank(query: &str, tool: &PromptToolDefinition, category: &str) -> Option<u8> {
    let name = tool.name.to_ascii_lowercase();
    if name == query {
        return Some(0);
    }
    if name.starts_with(query) || name.contains(query) {
        return Some(1);
    }
    if category == query || category.replace('_', " ") == query {
        return Some(2);
    }
    // An exact-looking operation is an identity query, not a search through
    // another tool's documentation mentioning that operation.
    if (query.contains('.') || query.contains('_')) && !query.chars().any(char::is_whitespace) {
        return None;
    }
    let normalized_query = normalize_intent(query);
    if tool_search_aliases(&tool.name)
        .iter()
        .any(|alias| normalize_intent(alias) == normalized_query)
    {
        return Some(3);
    }
    let haystack = format!(
        "{name} {category} {} {}",
        tool.description.to_ascii_lowercase(),
        tool_search_aliases(&tool.name).join(" ")
    );
    query
        .split_whitespace()
        .all(|token| haystack.contains(token))
        .then_some(4)
}

fn normalize_intent(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '-' | '_' | '/' => ' ',
            other => other.to_ascii_lowercase(),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn invalid_search() -> IpcError {
    IpcError::invalid_request(
        "orchestration.search_tools requires one to four non-empty queries of at most 96 bytes",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str) -> PromptToolDefinition {
        PromptToolDefinition {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }

    #[test]
    fn exact_operation_search_does_not_match_mentions_in_another_tools_description()
    -> Result<(), IpcError> {
        let reader = tool(
            "filesystem.inspect_image",
            "Read an image then use filesystem.write_delimited_table or filesystem.write_text to save it.",
        );
        for name in ["filesystem.write_delimited_table", "filesystem.write_text"] {
            for query in [name, name.strip_prefix("filesystem.").unwrap_or(name)] {
                let call = ParsedToolCall {
                    call_id: "search".into(),
                    recipient: SEARCH_TOOLS_TOOL.into(),
                    name: SEARCH_TOOLS_TOOL.into(),
                    arguments: std::collections::BTreeMap::from([(
                        "queries".into(),
                        serde_json::json!([query]),
                    )]),
                };
                assert!(
                    search_tool_definitions(&call, std::slice::from_ref(&reader))?
                        .loaded_names
                        .is_empty()
                );
                let writer = tool(name, "Write a file");
                assert_eq!(
                    search_tool_definitions(&call, &[reader.clone(), writer])?.loaded_names,
                    [name]
                );
            }
        }
        Ok(())
    }

    #[test]
    fn exact_and_capability_search_load_task_tools_without_controls() -> Result<(), IpcError> {
        let catalogue = [
            tool("filesystem.inspect_image", "Inspect a static image"),
            tool(
                "filesystem.write_delimited_table",
                "Create a CSV or TSV table",
            ),
            tool("orchestration.submit_plan", "Submit a plan"),
        ];
        let call = ParsedToolCall {
            call_id: "search".to_owned(),
            recipient: SEARCH_TOOLS_TOOL.to_owned(),
            name: SEARCH_TOOLS_TOOL.to_owned(),
            arguments: std::collections::BTreeMap::from([(
                "queries".to_owned(),
                serde_json::json!(["read image", "filesystem.write_delimited_table"]),
            )]),
        };
        let outcome = search_tool_definitions(&call, &catalogue)?;
        assert_eq!(
            outcome.loaded_names,
            [
                "filesystem.write_delimited_table",
                "filesystem.inspect_image"
            ]
        );
        assert!(
            !outcome
                .loaded_names
                .iter()
                .any(|name| name == "orchestration.submit_plan")
        );
        Ok(())
    }

    #[test]
    fn image_to_csv_intent_promotes_one_structured_consumer_and_the_reader() -> Result<(), IpcError>
    {
        let catalogue = [
            tool(
                "filesystem.inspect_image",
                "Read an image and route table values to the structured writer",
            ),
            tool(
                "filesystem.write_delimited_table",
                "Write top-level native JSON columns and rows arrays as CSV or TSV",
            ),
            tool("filesystem.write_text", "Write a generic text file"),
            tool("orchestration.submit_plan", "Submit a plan"),
        ];
        let call = ParsedToolCall {
            call_id: "image-to-csv-search".to_owned(),
            recipient: SEARCH_TOOLS_TOOL.to_owned(),
            name: SEARCH_TOOLS_TOOL.to_owned(),
            arguments: std::collections::BTreeMap::from([(
                "queries".to_owned(),
                serde_json::json!(["image to CSV"]),
            )]),
        };

        let outcome = search_tool_definitions(&call, &catalogue)?;

        assert_eq!(
            outcome.loaded_names,
            [
                "filesystem.write_delimited_table",
                "filesystem.inspect_image"
            ]
        );
        assert!(
            !outcome
                .loaded_names
                .contains(&"filesystem.write_text".to_owned())
        );
        assert_eq!(
            outcome.value["matches"][0]["schemaLoadedForNextRound"],
            true
        );
        assert!(
            outcome.value["matches"][0]["summary"]
                .as_str()
                .is_some_and(|summary| summary.contains("columns") && summary.contains("rows"))
        );
        Ok(())
    }

    #[test]
    fn image_to_file_intent_prefers_the_generic_writer_without_hiding_the_reader()
    -> Result<(), IpcError> {
        let catalogue = [
            tool("filesystem.inspect_image", "Read a static image"),
            tool("filesystem.write_text", "Create a generic text file"),
            tool(
                "filesystem.write_delimited_table",
                "Create a CSV or TSV table",
            ),
        ];
        let call = ParsedToolCall {
            call_id: "image-to-file-search".to_owned(),
            recipient: SEARCH_TOOLS_TOOL.to_owned(),
            name: SEARCH_TOOLS_TOOL.to_owned(),
            arguments: std::collections::BTreeMap::from([(
                "queries".to_owned(),
                serde_json::json!(["image-to-file"]),
            )]),
        };

        let outcome = search_tool_definitions(&call, &catalogue)?;

        assert_eq!(
            outcome.loaded_names,
            ["filesystem.write_text", "filesystem.inspect_image"]
        );
        Ok(())
    }
}
