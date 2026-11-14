//! The output fixture was produced by the real pinned, attested compiler test.
use super::super::*;
use crate::{CompilationInput, ReproducedArtifact};
fn fixture() -> Result<(CompilationInput, Value), Box<dyn std::error::Error>> {
    Ok((
        CompilationInput::new(
            "0.8.38",
            "ImmutableReview.sol",
            "ImmutableReview",
            serde_json::from_str(include_str!("../../../tests/fixtures/immutable-input.json"))?,
        )?,
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/immutable-output.json"
        ))?,
    ))
}
fn ranges(output: &Value) -> &Value {
    &output["contracts"]["ImmutableReview.sol"]["ImmutableReview"]["evm"]["deployedBytecode"]["immutableReferences"]
}
fn template(output: &Value) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(crate::bytecode::decode(output["contracts"]["ImmutableReview.sol"]["ImmutableReview"]["evm"]["deployedBytecode"]["object"].as_str().ok_or("missing compiled runtime")?)?)
}
fn fill(
    output: &Value,
    runtime: &mut [u8],
    id: i64,
    word: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    for range in ranges(output)
        .get(id.to_string())
        .and_then(Value::as_array)
        .ok_or("missing immutable ranges")?
    {
        let start = usize::try_from(range["start"].as_u64().ok_or("missing range")?)?;
        runtime
            .get_mut(start..start + 32)
            .ok_or("range outside runtime")?
            .copy_from_slice(&word);
    }
    Ok(())
}
#[test]
fn actual_compiler_fixture_observed_values_are_bound_without_constructor_invariants()
-> Result<(), Box<dyn std::error::Error>> {
    let (input, output) = fixture()?;
    let zero = template(&output)?;
    let first = ReproducedArtifact::for_runtime(&input, &output, &zero)?;
    assert_eq!(first.immutable_bindings().len(), 6);
    assert!(ReproducedArtifact::from_compiler(&input, &output).is_err());
    let (&cap, _) = first
        .immutable_bindings()
        .iter()
        .find(|(_, v)| v.solidity_type() == "uint256")
        .ok_or("missing cap")?;
    let mut runtime = zero.clone();
    fill(&output, &mut runtime, cap, U256::MAX.to_be_bytes())?;
    let changed = ReproducedArtifact::for_runtime(&input, &output, &runtime)?;
    assert_eq!(
        changed.immutable_bindings()[&cap].value(),
        &json!(U256::MAX.to_string())
    );
    assert_ne!(first.commitment(), changed.commitment());
    assert_eq!(changed.runtime(), runtime);
    assert_eq!(changed.source_ast(), first.source_ast());
    Ok(())
}
#[test]
fn actual_compiler_type_padding_and_every_unlisted_byte_remain_strict()
-> Result<(), Box<dyn std::error::Error>> {
    let (input, output) = fixture()?;
    let zero = template(&output)?;
    let artifact = ReproducedArtifact::for_runtime(&input, &output, &zero)?;
    for (&id, binding) in artifact.immutable_bindings() {
        let mut dirty = [0; 32];
        match binding.canonical_abi_type() {
            "bool" => dirty[31] = 2,
            "address" => dirty[0] = 1,
            "bytes4" => dirty[31] = 1,
            "int8" => dirty[30] = 1,
            "uint256" => continue,
            _ => return Err("unexpected fixture type".into()),
        }
        let mut changed = zero.clone();
        fill(&output, &mut changed, id, dirty)?;
        assert!(ReproducedArtifact::for_runtime(&input, &output, &changed).is_err());
    }
    let mut listed = vec![false; zero.len()];
    for ranges in ranges(&output)
        .as_object()
        .ok_or("missing references")?
        .values()
    {
        for range in ranges.as_array().ok_or("missing ranges")? {
            let start = usize::try_from(range["start"].as_u64().ok_or("missing start")?)?;
            listed
                .get_mut(start..start + 32)
                .ok_or("out of range")?
                .fill(true);
        }
    }
    for (index, listed) in listed.iter().enumerate() {
        if !listed {
            let mut changed = zero.clone();
            changed[index] ^= 1;
            assert!(ReproducedArtifact::for_runtime(&input, &output, &changed).is_err());
        }
    }
    Ok(())
}
#[test]
fn target_matching_plain_runtime_preserves_original_commitment_and_rejects_links()
-> Result<(), Box<dyn std::error::Error>> {
    let input = CompilationInput::new(
        "0.8.38",
        "Counter.sol",
        "Counter",
        serde_json::from_str(include_str!("../../../tests/fixtures/counter-input.json"))?,
    )?;
    let output: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/counter-output.json"))?;
    let plain = ReproducedArtifact::from_compiler(&input, &output)?;
    assert_eq!(
        plain.commitment(),
        ReproducedArtifact::for_runtime(&input, &output, plain.runtime())?.commitment()
    );
    assert!(plain.immutable_bindings().is_empty());
    let (input, output) = fixture()?;
    let runtime = template(&output)?;
    for code in ["bytecode", "deployedBytecode"] {
        let mut linked = output.clone();
        linked["contracts"]["ImmutableReview.sol"]["ImmutableReview"]["evm"][code]["linkReferences"] =
            json!({"library.sol":{"L":[{"start":1,"length":20}]}});
        assert!(ReproducedArtifact::for_runtime(&input, &linked, &runtime).is_err());
    }
    Ok(())
}
