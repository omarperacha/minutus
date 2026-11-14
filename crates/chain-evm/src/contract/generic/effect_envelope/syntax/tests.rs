use super::*;
use serde_json::json;
use solidity_compilation::{CompilationInput, TrustedCompiler};
use std::sync::atomic::AtomicBool;
fn metadata() -> Value {
    json!({"settings":{"compilationTarget":{"Dispatch.sol":"D"}}})
}
fn fixture() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(include_str!(
        "tests/fixtures/dispatch-ast.json"
    ))?)
}
fn contract<'a>(asts: &'a Value, name: &str) -> Result<&'a Value, Box<dyn std::error::Error>> {
    asts["Dispatch.sol"]["nodes"]
        .as_array()
        .and_then(|v| {
            v.iter()
                .find(|v| v["nodeType"] == "ContractDefinition" && v["name"] == name)
        })
        .ok_or_else(|| "missing fixture contract".into())
}
fn member<'a>(
    asts: &'a Value,
    owner: &str,
    name: &str,
) -> Result<&'a Value, Box<dyn std::error::Error>> {
    contract(asts, owner)?["nodes"]
        .as_array()
        .and_then(|v| v.iter().find(|v| v["name"] == name))
        .ok_or_else(|| "missing fixture member".into())
}
fn call(function: &Value) -> Result<&Value, Box<dyn std::error::Error>> {
    function["body"]["statements"][0]["expression"]
        .get("expression")
        .ok_or_else(|| "missing fixture call".into())
}
#[test]
fn compiled_diamond_virtual_super_explicit_and_nested_calls_resolve_by_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let asts = fixture()?;
    let program = Program::new(&asts, &metadata())?;
    let a = member(&asts, "A", "step")?;
    let b = member(&asts, "B", "step")?;
    let c = member(&asts, "C", "step")?;
    let d = member(&asts, "D", "step")?;
    assert_eq!(
        program.contracts,
        [
            id(contract(&asts, "D")?)?,
            id(contract(&asts, "C")?)?,
            id(contract(&asts, "B")?)?,
            id(contract(&asts, "A")?)?
        ]
    );
    let entry = member(&asts, "D", "entry")?;
    assert_eq!(
        id(program.resolve_helper(call(entry)?, Some(id(entry)?))?)?,
        id(d)?
    );
    assert_eq!(id(program.resolve_helper(call(d)?, Some(id(d)?))?)?, id(c)?);
    assert_eq!(id(program.resolve_helper(call(c)?, Some(id(c)?))?)?, id(b)?);
    assert_eq!(id(program.resolve_helper(call(b)?, Some(id(b)?))?)?, id(a)?);
    let inherited = member(&asts, "A", "inherited")?;
    assert_eq!(
        id(program.resolve_helper(call(inherited)?, Some(id(inherited)?))?)?,
        id(d)?
    );
    let base = member(&asts, "D", "viaBase")?;
    assert_eq!(
        id(program.resolve_helper(call(base)?, Some(id(base)?))?)?,
        id(a)?
    );
    let nested = member(&asts, "D", "viaNested")?;
    let relay = member(&asts, "A", "relay")?;
    assert_eq!(
        id(program.resolve_helper(call(nested)?, Some(id(nested)?))?)?,
        id(relay)?
    );
    assert_eq!(
        id(program.resolve_helper(call(relay)?, Some(id(relay)?))?)?,
        id(d)?
    );
    let inherited_public = member(&asts, "A", "inheritedPublic")?;
    assert_eq!(
        id(program.resolve_helper(call(inherited_public)?, Some(id(inherited_public)?))?)?,
        id(member(&asts, "D", "publicStep")?)?
    );
    let public = member(&asts, "D", "viaPublic")?;
    assert_eq!(
        id(program.resolve_helper(call(public)?, Some(id(public)?))?)?,
        id(member(&asts, "D", "publicStep")?)?
    );
    let private = member(&asts, "A", "inheritedPrivate")?;
    assert_eq!(
        id(program.resolve_helper(call(private)?, Some(id(private)?))?)?,
        id(member(&asts, "A", "privateStep")?)?
    );
    Ok(())
}
#[test]
fn compiled_fields_this_function_values_and_unscoped_super_do_not_become_static_helpers()
-> Result<(), Box<dyn std::error::Error>> {
    let asts = fixture()?;
    let program = Program::new(&asts, &metadata())?;
    for name in ["shadowedField", "viaThis", "pointer"] {
        let function = member(&asts, "D", name)?;
        assert!(
            program
                .resolve_helper(call(function)?, Some(id(function)?))
                .is_err()
        );
    }
    let step = member(&asts, "D", "step")?;
    assert!(program.resolve_helper(call(step)?, None).is_err());
    let private = member(&asts, "A", "inheritedPrivate")?;
    assert!(
        program
            .resolve_helper(call(private)?, Some(id(contract(&asts, "D")?)?))
            .is_err()
    );
    let inherited = member(&asts, "A", "inherited")?;
    let name = &inherited["modifiers"][0]["modifierName"];
    assert_eq!(
        id(program.resolve_modifier(name, Some(id(inherited)?))?)?,
        id(member(&asts, "D", "gate")?)?
    );
    let mut qualified = name.clone();
    qualified["name"] = json!("A.gate");
    assert!(
        program
            .resolve_modifier(&qualified, Some(id(inherited)?))
            .is_err()
    );
    assert!(program.resolve_modifier(name, None).is_err());
    let explicit = member(&asts, "D", "viaExplicitModifier")?;
    assert!(
        program
            .resolve_modifier(
                &explicit["modifiers"][0]["modifierName"],
                Some(id(explicit)?)
            )
            .is_err()
    );

    Ok(())
}
#[test]
#[ignore = "requires embedded normal compiler pin and attested sibling helper"]
fn genuine_compiler_dispatch_ast_fixture_matches() -> Result<(), Box<dyn std::error::Error>> {
    let compiler = std::fs::read(
        std::env::var_os("MINUTUS_TEST_OFFICIAL_SOLC_JS")
            .ok_or("missing official compiler path")?,
    )?;
    let input = CompilationInput::new(
        "0.8.38",
        "Dispatch.sol",
        "D",
        json!({"language":"Solidity","sources":{"Dispatch.sol":{"content":include_str!("tests/fixtures/Dispatch.sol")}},"settings":{"evmVersion":"cancun","optimizer":{"enabled":true,"runs":200},"viaIR":false}}),
    )?;
    let artifact =
        TrustedCompiler::packaged()?.reproduce(&input, &compiler, &AtomicBool::new(false))?;
    if let Some(path) = std::env::var_os("MINUTUS_TEST_DISPATCH_AST") {
        std::fs::write(path, serde_json::to_vec_pretty(artifact.source_ast())?)?;
    }
    assert!(
        artifact.source_ast() == &fixture()?,
        "actual compiler AST differs from fixture"
    );
    Program::new(artifact.source_ast(), artifact.metadata())?;
    Ok(())
}

#[test]
fn legacy_builtin_ids_normalize_only_exact_reviewed_names_and_cannot_be_declarations()
-> Result<(), Box<dyn std::error::Error>> {
    for (name, signed) in [
        ("assert", -3i64),
        ("msg", -15),
        ("require", -18),
        ("revert", -19),
        ("super", -25),
        ("tx", -26),
        ("this", -28),
    ] {
        assert_eq!(
            reference(&json!({"name":name,"referencedDeclaration":LEGACY_ID_MODULUS+signed}))?,
            signed
        );
        assert_eq!(
            reference(&json!({"name":name,"referencedDeclaration":signed}))?,
            signed
        );
        assert_eq!(
            reference(&json!({"name":name,"referencedDeclaration":42}))?,
            42
        );
    }
    for (name, reference_id) in [
        ("require", LEGACY_ID_MODULUS - 15),
        ("fake", LEGACY_ID_MODULUS - 18),
        ("require", MAX_AST_ID + 1),
        ("require", LEGACY_ID_MODULUS),
        ("msg", -18),
    ] {
        assert!(reference(&json!({"name":name,"referencedDeclaration":reference_id})).is_err());
    }
    let mut asts = fixture()?;
    asts["Dispatch.sol"]["nodes"][0]["id"] = json!(LEGACY_ID_MODULUS - 18);
    assert!(Program::new(&asts, &metadata()).is_err());
    Ok(())
}
