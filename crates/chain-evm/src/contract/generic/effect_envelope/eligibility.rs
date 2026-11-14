//! Feature-based historical compiler eligibility. Original settings are sealed
//! compiler inputs; historically lossy metadata is not a settings authority.
use super::{GenericError, analysis::modern, syntax::Program};
use serde_json::Value;
use solidity_compilation::ReproducedArtifact;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Features {
    Structured,
    LegacyScalar,
}

pub(super) fn admit(
    artifact: &ReproducedArtifact,
    program: &Program<'_>,
) -> Result<Features, GenericError> {
    if modern(artifact.compiler_version())? {
        return Ok(Features::Structured);
    }
    legacy_settings(artifact.compilation_settings())?;
    for unit in artifact
        .source_ast()
        .as_object()
        .ok_or(GenericError::OpaqueEffects)?
        .values()
    {
        if unit["nodes"].as_array().is_none_or(|nodes| {
            nodes.iter().any(|node| {
                node["nodeType"] == "PragmaDirective"
                    && node["literals"]
                        .as_array()
                        .is_some_and(|parts| parts.iter().any(|part| part == "experimental"))
            })
        }) {
            return Err(GenericError::OpaqueEffects);
        }
    }
    if !program.single_contract() || !artifact.immutable_bindings().is_empty() {
        return Err(GenericError::OpaqueEffects);
    }
    let target = artifact.metadata()["settings"]["compilationTarget"]
        .as_object()
        .filter(|v| v.len() == 1)
        .ok_or(GenericError::OpaqueEffects)?;
    let (file, name) = target.iter().next().ok_or(GenericError::OpaqueEffects)?;
    let contract = artifact.source_ast()[file]["nodes"]
        .as_array()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|node| node["nodeType"] == "ContractDefinition" && node["name"] == *name)
        })
        .ok_or(GenericError::OpaqueEffects)?;
    legacy_shape(contract, 0)?;
    for function in artifact.abi().as_array().ok_or(GenericError::InvalidAbi)? {
        for key in ["inputs", "outputs"] {
            if let Some(parameters) = function[key].as_array() {
                for parameter in parameters {
                    if !scalar(parameter["type"].as_str().unwrap_or("")) {
                        return Err(GenericError::OpaqueEffects);
                    }
                }
            }
        }
    }
    let layout = artifact.storage_layout();
    let Some(types) = layout["types"].as_object() else {
        return if layout["types"].is_null()
            && layout["storage"].as_array().is_some_and(Vec::is_empty)
        {
            Ok(Features::LegacyScalar)
        } else {
            Err(GenericError::OpaqueEffects)
        };
    };
    for spec in types.values() {
        match spec["encoding"].as_str() {
            Some("inplace")
                if !spec["members"].is_array()
                    && !spec["base"].is_string()
                    && scalar(spec["label"].as_str().unwrap_or("")) => {}
            Some("mapping") => {
                let types = &artifact.storage_layout()["types"];
                let key = &types[spec["key"].as_str().ok_or(GenericError::OpaqueEffects)?];
                let value = &types[spec["value"].as_str().ok_or(GenericError::OpaqueEffects)?];
                if key["encoding"] != "inplace"
                    || !scalar(key["label"].as_str().unwrap_or(""))
                    || value["encoding"] != "inplace"
                    || !value["label"]
                        .as_str()
                        .is_some_and(|ty| ty.starts_with("uint"))
                {
                    return Err(GenericError::OpaqueEffects);
                }
            }
            _ => return Err(GenericError::OpaqueEffects),
        }
    }
    Ok(Features::LegacyScalar)
}
fn legacy_settings(settings: &Value) -> Result<(), GenericError> {
    if settings["optimizer"]["enabled"] != false
        || settings["optimizer"].get("details").is_some()
        || settings.get("viaIR").is_some_and(|value| value != false)
        || settings.get("experimental").is_some()
        || settings.get("eofVersion").is_some()
    {
        return Err(GenericError::OpaqueEffects);
    }
    Ok(())
}
fn legacy_shape(node: &Value, depth: usize) -> Result<(), GenericError> {
    if depth > 64 {
        return Err(GenericError::Limit);
    }
    if matches!(
        node["nodeType"].as_str(),
        Some(
            "InlineAssembly"
                | "UncheckedBlock"
                | "ArrayTypeName"
                | "StructDefinition"
                | "EnumDefinition"
                | "UserDefinedValueTypeDefinition"
                | "FunctionTypeName"
                | "StorageLayoutSpecifier"
                | "ForStatement"
                | "WhileStatement"
                | "DoWhileStatement"
                | "TryStatement"
                | "NewExpression"
        )
    ) || node["operator"] == "delete"
        || node["storageLocation"] == "transient"
        || node["mutability"] == "immutable"
    {
        return Err(GenericError::OpaqueEffects);
    }
    if node["nodeType"] == "VariableDeclaration" {
        let ty = node["typeDescriptions"]["typeString"]
            .as_str()
            .ok_or(GenericError::OpaqueEffects)?;
        if !scalar(ty) && node["typeName"]["nodeType"] != "Mapping" {
            return Err(GenericError::OpaqueEffects);
        }
    }
    match node {
        Value::Object(object) => {
            for value in object.values() {
                legacy_shape(value, depth + 1)?;
            }
        }
        Value::Array(array) => {
            for value in array {
                legacy_shape(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn external_abi(function: &Value) -> Result<(), GenericError> {
    for key in ["parameters", "returnParameters"] {
        for parameter in function[key]["parameters"]
            .as_array()
            .ok_or(GenericError::OpaqueEffects)?
        {
            if !scalar(
                parameter["typeDescriptions"]["typeString"]
                    .as_str()
                    .unwrap_or(""),
            ) {
                return Err(GenericError::OpaqueEffects);
            }
        }
    }
    Ok(())
}
fn scalar(ty: &str) -> bool {
    // Compiler type strings carry reference/array suffixes after a contract
    // name or payable address. Never discard those suffixes before admission.
    if ty.contains(['[', ']', '(', ')']) {
        return false;
    }
    if matches!(ty, "address" | "address payable" | "bool") {
        return true;
    }
    if let Some(name) = ty.strip_prefix("contract ") {
        return !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'.'));
    }
    for prefix in ["uint", "int"] {
        if let Some(width) = ty.strip_prefix(prefix) {
            return width
                .parse::<u16>()
                .is_ok_and(|bits| (8..=256).contains(&bits) && bits % 8 == 0);
        }
    }
    ty.strip_prefix("bytes").is_some_and(|width| {
        width
            .parse::<u8>()
            .is_ok_and(|bytes| (1..=32).contains(&bytes))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn historical_settings_need_explicit_original_disabled_optimisation() -> Result<(), GenericError>
    {
        legacy_settings(&json!({"optimizer":{"enabled":false,"runs":200}}))?;
        for settings in [
            json!({}),
            json!({"optimizer":{}}),
            json!({"optimizer":{"enabled":true}}),
            json!({"optimizer":{"enabled":false,"details":{}}}),
            json!({"optimizer":{"enabled":false},"viaIR":true}),
            json!({"optimizer":{"enabled":false},"viaIR":"false"}),
            json!({"optimizer":{"enabled":false},"experimental":{}}),
        ] {
            assert!(legacy_settings(&settings).is_err());
        }
        Ok(())
    }
    #[test]
    fn scalar_eligibility_cannot_admit_reference_or_codegen_sensitive_constructs() {
        for kind in [
            "ArrayTypeName",
            "StructDefinition",
            "FunctionTypeName",
            "InlineAssembly",
            "UncheckedBlock",
            "StorageLayoutSpecifier",
            "ForStatement",
        ] {
            assert!(legacy_shape(&json!({"nodeType":kind}), 0).is_err());
        }
        assert!(scalar("address payable"));
        assert!(legacy_shape(&json!({"mutability":"immutable"}), 0).is_err());
        for ty in [
            "bytes",
            "uint256[]",
            "uint7",
            "int0",
            "function",
            "struct Pair",
            "contract ILeaf[2] memory",
            "contract ILeaf[] calldata",
            "address payable[] memory",
            "address memory",
            "contract",
        ] {
            assert!(!scalar(ty));
        }
    }
    #[test]
    fn historical_external_interface_checks_arguments_and_ignored_results() {
        let scalar_parameter = json!({"typeDescriptions":{"typeString":"uint256"}});
        let mut function = json!({"parameters":{"parameters":[scalar_parameter.clone()]},"returnParameters":{"parameters":[scalar_parameter]}});
        assert!(external_abi(&function).is_ok());
        function["returnParameters"]["parameters"][0]["typeDescriptions"]["typeString"] =
            json!("bytes memory");
        assert!(external_abi(&function).is_err());
        function["returnParameters"]["parameters"] = json!([]);
        function["parameters"]["parameters"][0]["typeDescriptions"]["typeString"] =
            json!("uint256[] calldata");
        assert!(external_abi(&function).is_err());
    }
}
