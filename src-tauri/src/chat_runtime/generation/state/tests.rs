use super::*;
use crate::chat_runtime::generation::recall_scope::RecallScope;

#[test]
fn startup_catalogue_describes_deferred_capabilities_without_exposing_them() {
    let all = [
        "orchestration.submit_plan",
        "orchestration.recall",
        "web.search",
        "computation.run",
        "filesystem.read_text",
    ]
    .map(|name| PromptToolDefinition {
        name: name.to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    });
    let mut state = GenerationToolState {
        recall_scope: RecallScope::CurrentInputOnly,
        ..GenerationToolState::default()
    };
    let index = state.available_catalogue_index(&all);
    let names = index
        .iter()
        .flat_map(app_core::ToolCategoryIndexEntry::operations)
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert!(names.contains("computation.run"));
    assert!(names.contains("filesystem.read_text"));
    assert!(!names.contains("filesystem.write_text"));
    assert!(!names.contains("orchestration.recall"));
    let callable = state.projected_tools(&all);
    assert!(
        callable
            .iter()
            .any(|tool| tool.name == "orchestration.submit_plan")
    );
    assert!(callable.iter().any(|tool| tool.name == "web.search"));
    assert!(!callable.iter().any(|tool| tool.name == "computation.run"));
    assert!(
        !callable
            .iter()
            .any(|tool| tool.name == "filesystem.read_text")
    );
    state.recall_scope = RecallScope::Historical;
    assert!(
        state
            .available_catalogue_index(&all)
            .iter()
            .any(|category| {
                category
                    .operations()
                    .iter()
                    .any(|name| name == "orchestration.recall")
            })
    );
}
