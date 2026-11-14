mod compiler;
use super::*;
fn ast(ty: &str) -> Value {
    json!({"file":{"id":1,"nodeType":"SourceUnit","nodes":[{"id":7,"nodeType":"VariableDeclaration","stateVariable":true,"mutability":"immutable","typeName":{"nodeType":"ElementaryTypeName","id":8,"name":ty},"typeDescriptions":{"typeString":ty}}]}})
}
#[test]
fn exact_mask_consistent_words_metadata_and_canonical_types_are_required()
-> Result<(), CompilationError> {
    let template = vec![0; 80];
    let refs = json!({"7":[{"start":1,"length":32},{"start":34,"length":32}]});
    let mut runtime = template.clone();
    runtime[32] = 42;
    runtime[65] = 42;
    let (_, bindings) = match_runtime(&template, Some(&runtime), &refs, &ast("uint256"))?;
    assert_eq!(bindings[&7].value(), &json!("42"));
    assert!(match_runtime(&template, None, &refs, &ast("uint256")).is_err());
    let mut changed = runtime.clone();
    changed[79] = 1;
    assert!(match_runtime(&template, Some(&changed), &refs, &ast("uint256")).is_err());
    changed = runtime.clone();
    changed[65] = 43;
    assert!(match_runtime(&template, Some(&changed), &refs, &ast("uint256")).is_err());
    for refs in [
        json!({"07":[{"start":1,"length":32}]}),
        json!({"9":[{"start":1,"length":32}]}),
        json!({"7":[]}),
        json!({"7":[{"start":1,"length":31}]}),
        json!({"7":[{"start":65,"length":32}]}),
        json!({"7":[{"start":1,"length":32},{"start":2,"length":32}]}),
    ] {
        assert!(match_runtime(&template, Some(&runtime), &refs, &ast("uint256")).is_err());
    }
    Ok(())
}
#[test]
fn full_width_and_narrow_signed_unsigned_fixed_bytes_and_addresses_are_canonical()
-> Result<(), CompilationError> {
    assert_eq!(
        canonical("uint256", [0xff; 32])?,
        json!(U256::MAX.to_string())
    );
    assert_eq!(canonical("int256", [0xff; 32])?, json!("-1"));
    assert_eq!(canonical("int8", [0xff; 32])?, json!("-1"));
    let mut negative = [0xff; 32];
    negative[31] = 0x80;
    assert_eq!(canonical("int8", negative)?, json!("-128"));
    negative[0] = 0;
    assert!(canonical("int8", negative).is_err());
    let mut unsigned = [0; 32];
    unsigned[31] = 0xff;
    assert_eq!(canonical("uint8", unsigned)?, json!("255"));
    unsigned[30] = 1;
    assert!(canonical("uint8", unsigned).is_err());
    let mut bytes = [0; 32];
    bytes[..4].copy_from_slice(&[1, 2, 3, 4]);
    assert_eq!(canonical("bytes4", bytes)?, json!("0x01020304"));
    bytes[31] = 1;
    assert!(canonical("bytes4", bytes).is_err());
    let mut address = [0; 32];
    address[12..].fill(0x11);
    assert_eq!(
        canonical("address", address)?,
        json!(format!("0x{}", "11".repeat(20)))
    );
    address[0] = 1;
    assert!(canonical("address", address).is_err());
    for ty in [
        "uint7",
        "uint0",
        "uint0256",
        "int7",
        "bytes0",
        "bytes33",
        "string",
        "enum E",
        "contract C",
    ] {
        assert!(canonical(ty, [0; 32]).is_err());
    }
    assert_eq!(canonical("bool", [0; 32])?, json!(false));
    let mut truth = [0; 32];
    truth[31] = 1;
    assert_eq!(canonical("bool", truth)?, json!(true));
    truth[31] = 2;
    assert!(canonical("bool", truth).is_err());
    Ok(())
}
#[test]
fn declaration_resolution_does_not_accept_constants_storage_or_unresolved_types() {
    let template = vec![0; 32];
    let refs = json!({"7":[{"start":0,"length":32}]});
    for (pointer, value) in [
        ("/file/nodes/0/mutability", json!("constant")),
        ("/file/nodes/0/stateVariable", json!(false)),
        (
            "/file/nodes/0/typeName/nodeType",
            json!("UserDefinedTypeName"),
        ),
        (
            "/file/nodes/0/typeDescriptions/typeString",
            json!("address"),
        ),
    ] {
        let mut ast = ast("uint256");
        if let Some(field) = ast.pointer_mut(pointer) {
            *field = value;
        }
        assert!(match_runtime(&template, Some(&template), &refs, &ast).is_err());
    }
}

#[test]
fn contract_typed_immutables_require_resolved_contract_ast_and_clean_address()
-> Result<(), CompilationError> {
    let mut ast = ast("uint256");
    ast["file"]["nodes"][0]["typeName"] =
        json!({"nodeType":"UserDefinedTypeName","id":8,"referencedDeclaration":99});
    ast["file"]["nodes"][0]["typeDescriptions"]["typeString"] = json!("contract Receiver");
    ast["file"]["nodes"]
        .as_array_mut()
        .ok_or(CompilationError::ArtifactMismatch)?
        .push(json!({"nodeType":"ContractDefinition","id":99,"name":"Receiver"}));
    let template = vec![0; 32];
    let refs = json!({"7":[{"start":0,"length":32}]});
    let mut runtime = template.clone();
    runtime[12..].fill(0x11);
    let (_, bindings) = match_runtime(&template, Some(&runtime), &refs, &ast)?;
    assert_eq!(bindings[&7].solidity_type(), "contract Receiver");
    assert_eq!(bindings[&7].canonical_abi_type(), "address");
    assert_eq!(
        bindings[&7].value(),
        &json!(format!("0x{}", "11".repeat(20)))
    );
    runtime[0] = 1;
    assert!(match_runtime(&template, Some(&runtime), &refs, &ast).is_err());
    for kind in [
        "EnumDefinition",
        "UserDefinedValueTypeDefinition",
        "FunctionDefinition",
    ] {
        let mut changed = ast.clone();
        changed["file"]["nodes"][1]["nodeType"] = json!(kind);
        assert!(match_runtime(&template, Some(&template), &refs, &changed).is_err());
    }
    ast["file"]["nodes"][0]["typeName"]["referencedDeclaration"] = json!(100);
    assert!(match_runtime(&template, Some(&template), &refs, &ast).is_err());
    Ok(())
}
#[test]
fn int256_minimum_and_nonzero_template_words_are_rejected_or_canonical()
-> Result<(), CompilationError> {
    let mut minimum = [0; 32];
    minimum[0] = 0x80;
    assert_eq!(
        canonical("int256", minimum)?,
        json!("-57896044618658097711785492504343953926634992332820282019728792003956564819968")
    );
    let mut template = vec![0; 32];
    template[0] = 1;
    let refs = json!({"7":[{"start":0,"length":32}]});
    assert!(match_runtime(&template, Some(&template), &refs, &ast("uint256")).is_err());
    Ok(())
}
