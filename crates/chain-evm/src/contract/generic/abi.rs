//! Compiler-derived ABI binding, separate from execution and business meaning.
use super::error::GenericError;
use crate::contract::{DeclaredContractCall, UnsignedContractIntent};
use alloy_primitives::keccak256;
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn bind(abi: &Value, intent: &UnsignedContractIntent) -> Result<(), GenericError> {
    let functions = abi
        .as_array()
        .filter(|v| v.len() <= 1024)
        .ok_or(GenericError::InvalidAbi)?;
    let mut selectors = BTreeSet::new();
    let mut selected = None;
    for function in functions.iter().filter(|v| v["type"] == "function") {
        let signature = signature(function)?;
        let selector = keccak256(signature.as_bytes())[..4].to_vec();
        if !selectors.insert(selector) {
            return Err(GenericError::InvalidAbi);
        }
        if signature == intent.call().signature() {
            selected = Some(function);
        }
    }
    let function = selected.ok_or(GenericError::InvalidAbi)?;
    if !matches!(
        function["stateMutability"].as_str(),
        Some("nonpayable" | "payable")
    ) || (intent.value_wei() != "0" && function["stateMutability"] != "payable")
    {
        return Err(GenericError::InvalidAbi);
    }
    let outputs = parameters(&function["outputs"])?;
    if outputs != intent.call().outputs() {
        return Err(GenericError::InvalidAbi);
    }
    let call = DeclaredContractCall::new(
        intent.call().target(),
        intent.call().signature(),
        intent.call().arguments(),
        &outputs,
    )
    .map_err(|_| GenericError::InvalidAbi)?;
    if call.calldata() != intent.call().calldata() {
        return Err(GenericError::InvalidAbi);
    }
    Ok(())
}

pub(super) fn signature(item: &Value) -> Result<String, GenericError> {
    let name = item["name"].as_str().ok_or(GenericError::InvalidAbi)?;
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
    {
        return Err(GenericError::InvalidAbi);
    }
    Ok(format!(
        "{name}({})",
        parameters(&item["inputs"])?.join(",")
    ))
}

pub(super) fn parameters(value: &Value) -> Result<Vec<String>, GenericError> {
    let items = value
        .as_array()
        .filter(|v| v.len() <= 16)
        .ok_or(GenericError::InvalidAbi)?;
    items.iter().map(|item| parameter(item, 0)).collect()
}
fn parameter(item: &Value, depth: usize) -> Result<String, GenericError> {
    if depth > 4 {
        return Err(GenericError::Limit);
    }
    let ty = item["type"]
        .as_str()
        .filter(|v| v.len() <= 512)
        .ok_or(GenericError::InvalidAbi)?;
    if let Some(suffix) = ty.strip_prefix("tuple") {
        if !suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'[' || b == b']')
        {
            return Err(GenericError::InvalidAbi);
        }
        let members = item["components"]
            .as_array()
            .filter(|v| v.len() <= 16)
            .ok_or(GenericError::InvalidAbi)?;
        let members = members
            .iter()
            .map(|v| parameter(v, depth + 1))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(format!("({}){suffix}", members.join(",")))
    } else {
        Ok(ty.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EvmNetwork;
    use serde_json::json;
    #[test]
    fn arbitrary_box_counter_and_overloads_bind_but_deceptive_outputs_do_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let abi = json!([
            {"type":"function","name":"store","inputs":[{"type":"uint256"}],"outputs":[],"stateMutability":"nonpayable"},
            {"type":"function","name":"increment","inputs":[],"outputs":[{"type":"uint256"}],"stateMutability":"nonpayable"},
            {"type":"function","name":"store","inputs":[{"type":"bool"}],"outputs":[],"stateMutability":"nonpayable"}
        ]);
        for (signature, args, outputs) in [
            ("store(uint256)", vec![json!("42")], vec![]),
            ("increment()", vec![], vec!["uint256".into()]),
            ("store(bool)", vec![json!(true)], vec![]),
        ] {
            let call = DeclaredContractCall::new(
                "0x2222222222222222222222222222222222222222",
                signature,
                &args,
                &outputs,
            )?;
            let intent = UnsignedContractIntent::new_declared(
                EvmNetwork::EthereumSepolia,
                "0x1111111111111111111111111111111111111111",
                "0",
                call,
            )?;
            bind(&abi, &intent)?;
            let mut bad = abi.clone();
            bad[0]["outputs"] = json!([{"type":"bool"}]);
            if signature == "store(uint256)" {
                assert_eq!(bind(&bad, &intent), Err(GenericError::InvalidAbi));
            }
        }
        Ok(())
    }
    #[test]
    fn selector_collisions_and_duplicate_entries_reject() -> Result<(), Box<dyn std::error::Error>>
    {
        let first = json!({"type":"function","name":"burn","inputs":[{"type":"uint256"}],"outputs":[],"stateMutability":"nonpayable"});
        let second = json!({"type":"function","name":"collate_propagate_storage","inputs":[{"type":"bytes16"}],"outputs":[],"stateMutability":"nonpayable"});
        assert_eq!(
            &keccak256(signature(&first)?)[..4],
            &keccak256(signature(&second)?)[..4]
        );
        let call = DeclaredContractCall::new(
            "0x2222222222222222222222222222222222222222",
            "burn(uint256)",
            &[json!("1")],
            &[],
        )?;
        let intent = UnsignedContractIntent::new_declared(
            EvmNetwork::EthereumSepolia,
            "0x1111111111111111111111111111111111111111",
            "0",
            call,
        )?;
        assert_eq!(
            bind(&json!([first.clone(), first]), &intent),
            Err(GenericError::InvalidAbi)
        );
        assert_eq!(
            bind(
                &json!([second,{"type":"function","name":"burn","inputs":[{"type":"uint256"}],"outputs":[],"stateMutability":"nonpayable"}]),
                &intent
            ),
            Err(GenericError::InvalidAbi)
        );
        Ok(())
    }
}
