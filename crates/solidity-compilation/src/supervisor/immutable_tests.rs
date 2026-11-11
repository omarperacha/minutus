//! Genuine packaged compiler, exact immutable substitution and mutation probes.
use super::*;
use ruint::aliases::U256;
use serde_json::{Value, json};

fn input() -> Result<CompilationInput, CompilationError> {
    CompilationInput::new(
        "0.8.38",
        "ImmutableReview.sol",
        "ImmutableReview",
        serde_json::from_str(include_str!("../../tests/fixtures/immutable-input.json"))
            .map_err(|_| CompilationError::InvalidInput)?,
    )
}
fn declaration(value: &Value, id: i64) -> Option<&Value> {
    if value["nodeType"] == "VariableDeclaration" && value["id"].as_i64() == Some(id) {
        return Some(value);
    }
    match value {
        Value::Object(values) => values.values().find_map(|v| declaration(v, id)),
        Value::Array(values) => values.iter().find_map(|v| declaration(v, id)),
        _ => None,
    }
}
fn value(name: &str) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let mut word = [0; 32];
    match name {
        "cap" => word = U256::MAX.to_be_bytes(),
        "owner" => word[12..].fill(0x11),
        "receiver" => word[12..].fill(0x22),
        "tag" => word[..4].copy_from_slice(&[1, 2, 3, 4]),
        "signedLimit" => {
            word.fill(0xff);
            word[31] = 0x80;
        }
        "enabled" => word[31] = 1,
        _ => return Err("unexpected compiled immutable".into()),
    }
    Ok(word)
}
fn patched(output: &Value) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let evm =
        &output["contracts"]["ImmutableReview.sol"]["ImmutableReview"]["evm"]["deployedBytecode"];
    let mut runtime = crate::bytecode::decode(evm["object"].as_str().ok_or("missing runtime")?)?;
    for (id, ranges) in evm["immutableReferences"]
        .as_object()
        .ok_or("missing references")?
    {
        let node = declaration(&output["sources"], id.parse()?).ok_or("unresolved declaration")?;
        let word = value(node["name"].as_str().ok_or("missing declaration name")?)?;
        for range in ranges.as_array().ok_or("invalid references")? {
            let start = usize::try_from(range["start"].as_u64().ok_or("missing reference start")?)?;
            runtime
                .get_mut(start..start + 32)
                .ok_or("reference outside runtime")?
                .copy_from_slice(&word);
        }
    }
    Ok(runtime)
}
#[test]
#[ignore = "requires official solc JS, normal embedded pin and attested sibling compiler"]
fn genuine_compiler_runtime_bindings_match_and_mutations_reject()
-> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(
        std::env::var_os("MINUTUS_TEST_OFFICIAL_SOLC_JS")
            .ok_or("missing official solc test path")?,
    )?;
    let input = input()?;
    let compiler = TrustedCompiler::packaged()?;
    let cancelled = AtomicBool::new(false);
    let output = decode_output(
        &input,
        &compiler.compile_response(&input, &bytes, &cancelled)?,
    )?;
    if let Some(path) = std::env::var_os("MINUTUS_TEST_IMMUTABLE_OUTPUT") {
        fs::write(path, serde_json::to_vec_pretty(&output)?)?;
    }
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/immutable-output.json"))?;
    assert!(
        output == fixture,
        "pinned compiler output differs from fixture"
    );
    let runtime = patched(&output)?;
    let artifact = ReproducedArtifact::for_runtime(&input, &output, &runtime)?;
    assert_eq!(artifact.runtime(), runtime);
    assert_eq!(artifact.immutable_bindings().len(), 6);
    assert!(ReproducedArtifact::from_compiler(&input, &output).is_err());
    for (id, binding) in artifact.immutable_bindings() {
        assert_eq!(*id, binding.ast_id());
        let name = declaration(&output["sources"], *id)
            .and_then(|node| node["name"].as_str())
            .ok_or("missing name")?;
        assert_eq!(binding.word(), &value(name)?);
        if name == "receiver" {
            assert_eq!(binding.solidity_type(), "contract Receiver");
            assert_eq!(binding.canonical_abi_type(), "address");
        }
        if name == "cap" {
            assert_eq!(binding.value(), &json!(U256::MAX.to_string()));
        }
        if name == "signedLimit" {
            assert_eq!(binding.value(), &json!("-128"));
        }
    }
    let again = compiler.reproduce_for_runtime(&input, &bytes, &runtime, &cancelled)?;
    assert_eq!(artifact.commitment(), again.commitment());
    for index in [0, runtime.len() - 1] {
        let mut changed = runtime.clone();
        changed[index] ^= 1;
        assert!(ReproducedArtifact::for_runtime(&input, &output, &changed).is_err());
    }
    assert!(
        ReproducedArtifact::for_runtime(&input, &output, &runtime[..runtime.len() - 1]).is_err()
    );
    let refs = &output["contracts"]["ImmutableReview.sol"]["ImmutableReview"]["evm"]["deployedBytecode"]
        ["immutableReferences"];
    let duplicate = refs
        .as_object()
        .and_then(|values| {
            values
                .values()
                .find_map(|v| v.as_array().filter(|v| v.len() > 1))
        })
        .ok_or("fixture must test repeated immutable")?;
    let start = usize::try_from(
        duplicate[1]["start"]
            .as_u64()
            .ok_or("missing repeated start")?,
    )?;
    let mut changed = runtime.clone();
    changed[start + 31] ^= 1;
    assert!(ReproducedArtifact::for_runtime(&input, &output, &changed).is_err());
    if let Some(path) = std::env::var_os("MINUTUS_TEST_IMMUTABLE_OUTPUT") {
        fs::write(path, serde_json::to_vec_pretty(&output)?)?;
    }
    Ok(())
}

#[test]
#[ignore = "requires genuine packaged App Sandbox host and fixed compiler service cohort"]
fn genuine_packaged_compiler_three_sequential_sessions() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(
        std::env::var_os("MINUTUS_TEST_OFFICIAL_SOLC_JS")
            .ok_or("missing official solc test path")?,
    )?;
    let input = input()?;
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/immutable-output.json"))?;
    let runtime = patched(&fixture)?;
    let compiler = TrustedCompiler::packaged()?;
    let cancelled = AtomicBool::new(false);
    let mut commitment = None;
    for _ in 0..3 {
        let artifact = compiler.reproduce_for_runtime(&input, &bytes, &runtime, &cancelled)?;
        assert_eq!(artifact.runtime(), runtime);
        if let Some(previous) = &commitment {
            assert_eq!(artifact.commitment(), previous);
        }
        commitment = Some(artifact.commitment().to_owned());
    }
    Ok(())
}
