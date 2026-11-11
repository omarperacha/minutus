use super::*;
use serde_json::json;

fn fixture() -> Result<(CompilationInput, Value), Box<dyn std::error::Error>> {
    Ok((
        CompilationInput::new(
            "0.8.38",
            "Counter.sol",
            "Counter",
            serde_json::from_str(include_str!("../../tests/fixtures/counter-input.json"))?,
        )?,
        serde_json::from_str(include_str!("../../tests/fixtures/counter-output.json"))?,
    ))
}
fn rejected(input: &CompilationInput, output: &Value) {
    assert!(ReproducedArtifact::from_compiler(input, output).is_err());
}
#[test]
fn complete_real_compiler_fixture_is_deterministic_and_binds_input()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut input, output) = fixture()?;
    let first = ReproducedArtifact::from_compiler(&input, &output)?;
    let second = ReproducedArtifact::from_compiler(&input, &output)?;
    assert_eq!(first.commitment(), second.commitment());
    assert!(!first.runtime().is_empty());
    assert!(!first.creation().is_empty());
    assert_eq!(
        first.source_ast().as_object().map(serde_json::Map::len),
        Some(1)
    );
    input.standard["settings"]["optimizer"]["runs"] = json!(201);
    assert_ne!(
        first.commitment(),
        ReproducedArtifact::from_compiler(&input, &output)?.commitment()
    );
    Ok(())
}
#[test]
fn compiler_errors_wrong_target_and_incomplete_projection_reject()
-> Result<(), Box<dyn std::error::Error>> {
    let (input, output) = fixture()?;
    for pointer in [
        "/contracts/Counter.sol/Counter/abi",
        "/contracts/Counter.sol/Counter/storageLayout",
        "/contracts/Counter.sol/Counter/evm/methodIdentifiers",
        "/contracts/Counter.sol/Counter/metadata",
    ] {
        let mut changed = output.clone();
        *changed.pointer_mut(pointer).ok_or("missing fixture path")? = Value::Null;
        rejected(&input, &changed);
    }
    let mut changed = output.clone();
    changed["errors"] = json!([{"severity":"error","formattedMessage":"untrusted text"}]);
    rejected(&input, &changed);
    let mut changed = output;
    changed["contracts"]["Counter.sol"]["Counter"] = Value::Null;
    rejected(&input, &changed);
    Ok(())
}
#[test]
fn unlinked_and_immutable_runtime_is_not_accepted_as_reproduced()
-> Result<(), Box<dyn std::error::Error>> {
    let (input, output) = fixture()?;
    for pointer in [
        "/contracts/Counter.sol/Counter/evm/deployedBytecode/immutableReferences",
        "/contracts/Counter.sol/Counter/evm/deployedBytecode/linkReferences",
        "/contracts/Counter.sol/Counter/evm/bytecode/linkReferences",
    ] {
        for replacement in [
            json!({"foreign":[{"start":1,"length":20}]}),
            json!([]),
            Value::Null,
        ] {
            let mut changed = output.clone();
            *changed.pointer_mut(pointer).ok_or("missing fixture path")? = replacement;
            rejected(&input, &changed);
        }
    }
    Ok(())
}
#[test]
fn runtime_and_creation_require_canonical_bounded_hex() -> Result<(), Box<dyn std::error::Error>> {
    let (input, output) = fixture()?;
    for key in ["bytecode", "deployedBytecode"] {
        for value in [
            "0x00".into(),
            "AA".into(),
            "gg".into(),
            "0".into(),
            "00".repeat(49_153),
        ] {
            let mut changed = output.clone();
            changed["contracts"]["Counter.sol"]["Counter"]["evm"][key]["object"] = json!(value);
            rejected(&input, &changed);
        }
    }
    for value in [String::new(), "00".repeat(24_577)] {
        let mut changed = output.clone();
        changed["contracts"]["Counter.sol"]["Counter"]["evm"]["deployedBytecode"]["object"] =
            json!(value);
        rejected(&input, &changed);
    }
    Ok(())
}
#[test]
fn metadata_version_abi_target_and_source_ast_coverage_must_correspond()
-> Result<(), Box<dyn std::error::Error>> {
    let (input, output) = fixture()?;
    for (pointer, replacement) in [
        ("/compiler/version", json!("0.8.38+commit.deadbeef")),
        ("/output/abi", json!([])),
        ("/settings/compilationTarget/Counter.sol", json!("Other")),
    ] {
        let mut changed = output.clone();
        let mut metadata: Value = serde_json::from_str(
            changed["contracts"]["Counter.sol"]["Counter"]["metadata"]
                .as_str()
                .ok_or("metadata missing")?,
        )?;
        *metadata
            .pointer_mut(pointer)
            .ok_or("metadata path missing")? = replacement;
        changed["contracts"]["Counter.sol"]["Counter"]["metadata"] =
            json!(serde_json::to_string(&metadata)?);
        rejected(&input, &changed);
    }
    for sources in [
        json!({}),
        json!({"Other.sol":{"ast":{}}}),
        json!({"Counter.sol":{}}),
        json!({"Counter.sol":{"ast":[]}}),
        json!({"Counter.sol":{"ast":{}},"Other.sol":{"ast":{}}}),
    ] {
        let mut changed = output.clone();
        changed["sources"] = sources;
        rejected(&input, &changed);
    }
    let mut incomplete = input;
    incomplete.standard["sources"]["Unreturned.sol"] = json!({"content":"contract Unreturned {}"});
    rejected(&incomplete, &output);
    Ok(())
}

#[test]
fn original_compilation_settings_remain_sealed_when_metadata_omits_details()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut input, output) = fixture()?;
    input.standard["settings"]["optimizer"] =
        json!({"enabled":false,"details":{"peephole":false,"jumpdestRemover":false}});
    input.standard["settings"]["viaIR"] = json!(false);
    let expected = input.standard["settings"].clone();
    let artifact = ReproducedArtifact::from_compiler(&input, &output)?;
    assert_eq!(artifact.compilation_settings(), &expected);
    assert!(
        artifact.metadata()["settings"]["optimizer"]
            .get("details")
            .is_none()
    );
    input.standard["settings"]["optimizer"]["enabled"] = json!(true);
    assert_eq!(artifact.compilation_settings(), &expected);
    assert_ne!(
        artifact.commitment(),
        ReproducedArtifact::from_compiler(&input, &output)?.commitment()
    );
    Ok(())
}
