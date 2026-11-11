use super::*;
use crate::contract::generic::effect_envelope::syntax::Program;
use solidity_compilation::{CompilationInput, TrustedCompiler};
use std::{cell::Cell, collections::BTreeMap, sync::atomic::AtomicBool};
fn fixture() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(include_str!(
        "tests/fixtures/batch.json"
    ))?)
}
fn analyze(
    name: &str,
    args: &[Value],
) -> Result<Vec<super::super::GenericEffectRelation>, Box<dyn std::error::Error>> {
    let fixture = fixture()?;
    let asts = &fixture["asts"];
    let program = Program::new(asts, &fixture["metadata"])?;
    let function = asts["Batch.sol"]["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|v| v["name"] == "Batch"))
        .and_then(|contract| contract["nodes"].as_array())
        .and_then(|nodes| nodes.iter().find(|v| v["name"] == name))
        .ok_or("missing compiled function")?;
    let parameters = function["parameters"]["parameters"]
        .as_array()
        .ok_or("missing params")?;
    assert_eq!(parameters.len(), args.len());
    let values = parameters
        .iter()
        .zip(args)
        .map(|(parameter, value)| Ok((syntax::id(parameter)?, value.clone())))
        .collect::<Result<BTreeMap<_, _>, GenericError>>()?;
    let bindings = BTreeMap::new();
    let mut analysis = Analysis {
        program: &program,
        layout: &fixture["layout"],
        values,
        relations: Vec::new(),
        calls: Vec::new(),
        steps: Cell::new(0),
        modern: true,
        features: super::super::eligibility::Features::Structured,
        base: false,
        account: "0x1111111111111111111111111111111111111111",
        origin: "0x1111111111111111111111111111111111111111",
        target: "0x2222222222222222222222222222222222222222",
        code_address: "0x2222222222222222222222222222222222222222",
        immutable_bindings: &bindings,
        native_value: "0",
        terminated: false,
        stack: Vec::new(),
    };
    analysis.function(function, 0)?;
    Ok(analysis.relations)
}
#[test]
fn compiled_calldata_loops_and_checked_local_copies_produce_exact_relations()
-> Result<(), Box<dyn std::error::Error>> {
    let relations = analyze("batch", &[json!(["7", "9"]), json!(["11", "13"])])?;
    assert_eq!(relations.len(), 2);
    assert_eq!(relations[0].path, ["entries", "7"]);
    assert_eq!(relations[1].value, json!("13"));
    assert_eq!(
        analyze("range", &[json!("10"), json!("32"), json!("5")])?.len(),
        32
    );
    assert_eq!(analyze("bodyUpdate", &[json!("2")])?.len(), 2);
    assert_eq!(analyze("localCopy", &[json!("4")])?[0].value, json!("5"));
    assert_eq!(analyze("signedCopy", &[json!("-2")])?[0].value, json!("-1"));
    assert_eq!(analyze("boolCopy", &[json!(false)])?[0].value, json!(true));
    Ok(())
}
#[test]
fn compiled_unbounded_state_dependent_duplicate_and_overflow_paths_reject() {
    for (name, args) in [
        ("range", vec![json!("10"), json!("33"), json!("5")]),
        ("batch", vec![json!(["7", "7"]), json!(["11", "13"])]),
        ("arrayCopy", vec![json!(["1"]), json!(["2"])]),
        ("stateLoop", vec![json!("5")]),
        ("neverTerminates", vec![json!("5")]),
        ("localCopy", vec![json!(U256::MAX.to_string())]),
        ("signedCopy", vec![json!("127")]),
        ("breakLoop", vec![json!("3")]),
        ("uncheckedLocal", vec![json!("1")]),
    ] {
        assert!(analyze(name, &args).is_err(), "{name} must reject");
    }
}
#[test]
#[ignore = "requires normal embedded compiler pin and attested sibling helper"]
fn actual_compiler_batch_fixture_matches() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(
        std::env::var_os("MINUTUS_TEST_OFFICIAL_SOLC_JS").ok_or("missing compiler path")?,
    )?;
    let input = CompilationInput::new(
        "0.8.38",
        "Batch.sol",
        "Batch",
        json!({"language":"Solidity","sources":{"Batch.sol":{"content":include_str!("tests/fixtures/Batch.sol")}},"settings":{"optimizer":{"enabled":true,"runs":200},"viaIR":false,"evmVersion":"cancun"}}),
    )?;
    let artifact =
        TrustedCompiler::packaged()?.reproduce(&input, &bytes, &AtomicBool::new(false))?;
    let output = json!({"asts":artifact.source_ast(),"metadata":artifact.metadata(),"layout":artifact.storage_layout()});
    if let Some(path) = std::env::var_os("MINUTUS_TEST_BATCH_FIXTURE") {
        std::fs::write(path, serde_json::to_vec_pretty(&output)?)?;
    }
    assert!(
        output == fixture()?,
        "actual compiler batch output differs from fixture"
    );
    Ok(())
}
