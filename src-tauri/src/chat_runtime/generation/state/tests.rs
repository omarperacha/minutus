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

#[test]
fn fresh_chat_catalogue_and_callable_surface_include_bounded_transfer()
-> Result<(), ipc_types::IpcError> {
    let all = crate::chat_tools::chat_tool_definitions(&[])?;
    let state = GenerationToolState {
        recall_scope: RecallScope::CurrentInputOnly,
        ..GenerationToolState::default()
    };
    let index = state.available_catalogue_index(&all);
    let payments = index
        .iter()
        .find(|category| category.name() == "payments")
        .ok_or_else(ipc_types::IpcError::internal)?;
    assert!(
        payments
            .operations()
            .iter()
            .any(|name| name == "wallet.request_transfer")
    );
    let callable = state.projected_tools(&all);
    assert!(
        callable
            .iter()
            .any(|tool| tool.name == "wallet.request_transfer")
    );

    println!("MINUTUS_CATEGORY_SNAPSHOT={}", serde_json::json!(index.iter().map(|entry|
        serde_json::json!({"name":entry.name(),"responsibility":entry.responsibility(),"operations":entry.operations()})
    ).collect::<Vec<_>>()));
    Ok(())
}
