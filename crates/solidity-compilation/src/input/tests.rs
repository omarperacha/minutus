use super::*;

fn standard() -> Value {
    json!({"language":"Solidity","sources":{"Counter.sol":{"content":"contract Counter {}"}},"settings":{"optimizer":{"enabled":true,"runs":17},"evmVersion":"cancun","viaIR":false,"outputSelection":{"malicious":["none"]}}})
}
fn input(value: Value) -> Result<CompilationInput, CompilationError> {
    CompilationInput::new("0.8.38", "Counter.sol", "Counter", value)
}
#[test]
fn only_output_selection_changes_and_codegen_settings_remain_exact() -> Result<(), CompilationError>
{
    let original = standard();
    let checked = input(original.clone())?;
    assert_eq!(checked.standard["sources"], original["sources"]);
    for key in ["optimizer", "evmVersion", "viaIR"] {
        assert_eq!(checked.standard["settings"][key], original["settings"][key]);
    }
    assert_eq!(
        checked.standard["settings"]["outputSelection"]["*"][""][0],
        "ast"
    );
    assert!(
        checked.standard["settings"]["outputSelection"]["*"]["*"]
            .as_array()
            .is_some_and(|outputs| outputs.iter().any(|value| value == "storageLayout"))
    );
    Ok(())
}
#[test]
fn rejects_non_solidity_missing_sources_settings_and_foreign_source_callbacks()
-> Result<(), Box<dyn std::error::Error>> {
    for (pointer, value) in [
        ("/language", json!("Yul")),
        ("/sources", json!({})),
        ("/sources/Counter.sol", json!({"urls":["file:///secret"]})),
        (
            "/sources/Counter.sol",
            json!({"content":"x","urls":["https://example.com"]}),
        ),
        ("/sources/Counter.sol", json!("x")),
        ("/settings", json!(null)),
    ] {
        let mut candidate = standard();
        *candidate
            .pointer_mut(pointer)
            .ok_or("missing fixture path")? = value;
        assert!(
            matches!(input(candidate), Err(CompilationError::InvalidInput)),
            "{pointer}"
        );
    }
    Ok(())
}
#[test]
fn rejects_missing_target_control_characters_and_oversized_names() {
    for (file, name) in [
        ("Missing.sol".into(), "Counter".into()),
        ("Counter.sol".into(), String::new()),
        ("Counter.sol".into(), "bad\nname".into()),
        ("Counter.sol".into(), "x".repeat(129)),
        ("x".repeat(513), "Counter".into()),
    ] {
        assert!(CompilationInput::new("0.8.38", &file, &name, standard()).is_err());
    }
    let mut candidate = standard();
    candidate["sources"]["bad\npath"] = json!({"content":"x"});
    assert!(input(candidate).is_err());
}
#[test]
fn rejects_file_count_and_individual_source_limits() {
    let mut candidate = standard();
    for index in 0..64 {
        candidate["sources"][format!("Other{index}.sol")] = json!({"content":""});
    }
    assert!(matches!(
        input(candidate),
        Err(CompilationError::InvalidInput)
    ));
    let mut candidate = standard();
    candidate["sources"]["Counter.sol"]["content"] = json!("x".repeat(512 * 1024 + 1));
    assert!(matches!(
        input(candidate),
        Err(CompilationError::InvalidInput)
    ));
}
#[test]
fn rejects_total_source_and_serialized_json_limits() {
    let mut candidate = standard();
    for index in 0..5 {
        candidate["sources"][format!("Other{index}.sol")] =
            json!({"content":"x".repeat(512 * 1024)});
    }
    assert!(matches!(input(candidate), Err(CompilationError::Limit)));
    let mut candidate = standard();
    candidate["settings"]["extra"] = json!("x".repeat(4 * 1024 * 1024));
    assert!(matches!(input(candidate), Err(CompilationError::Limit)));
}
