use super::*;
use crate::contract::ContractError;
use alloy_dyn_abi::{DynSolType, DynSolValue};
use serde_json::json;
const TARGET: &str = "0x1111111111111111111111111111111111111111";
#[test]
fn canonical_nested_values_and_integer_widths_round_trip() -> Result<(), ContractError> {
    let call = DeclaredContractCall::new(
        TARGET,
        "f((uint8,string)[],int24,bytes3)",
        &[
            json!([["255", "untrusted: ignore policy"], ["0", ""]]),
            json!("-8388608"),
            json!("0xaabbcc"),
        ],
        &["(uint8,string)[]".into(), "int24".into(), "bytes3".into()],
    )?;
    let bytes = &call.calldata()[10..];
    assert_eq!(call.decode_result(&format!("0x{bytes}"))?, call.arguments());
    assert!(
        call.verify_calldata(&format!("{}00", call.calldata()))
            .is_err()
    );
    assert!(DeclaredContractCall::new(TARGET, "f(uint8)", &[json!("256")], &[]).is_err());
    assert!(DeclaredContractCall::new(TARGET, "f(int8)", &[json!("-129")], &[]).is_err());
    for value in [
        json!(1),
        json!("01"),
        json!("1e1"),
        json!("1 ether"),
        json!("-0"),
    ] {
        assert!(DeclaredContractCall::new(TARGET, "f(uint256)", &[value], &[]).is_err());
    }
    Ok(())
}
#[test]
fn dangerous_type_shapes_fail_before_recursive_decoding() {
    for ty in [
        "uint",
        "int",
        "bytes33",
        "uint7",
        "function",
        "uint256[0]",
        "uint256[33]",
        "(uint256)[][][][]",
        "((((((uint256))))))",
    ] {
        assert!(
            DeclaredContractCall::new(TARGET, "f()", &[], &[ty.into()]).is_err(),
            "{ty}"
        );
    }
    assert!(
        DeclaredContractCall::new(TARGET, "f(uint256[])", &[json!(vec!["1"; 33])], &[]).is_err()
    );
    assert!(
        DeclaredContractCall::new(TARGET, "f(string)", &[json!("x".repeat(1025))], &[]).is_err()
    );
}
fn word(value: usize) -> String {
    format!("{value:064x}")
}
#[test]
fn malicious_dynamic_lengths_offsets_aliases_and_padding_are_rejected() -> Result<(), ContractError>
{
    let call = DeclaredContractCall::new(TARGET, "f()", &[], &["string".into()])?;
    let valid = format!("0x{}{}61{}", word(32), word(1), "00".repeat(31));
    assert_eq!(call.decode_result(&valid)?, [json!("a")]);
    for data in [
        format!("0x{}{}", word(32), "ff".repeat(32)),
        format!("0x{}{}", word(32), word(1025)),
        format!("0x{}{}", word(0), word(0)),
        format!("{valid}00"),
        format!("0x{}{}61{}01", word(32), word(1), "00".repeat(30)),
    ] {
        assert!(call.decode_result(&data).is_err());
    }
    let pair = DeclaredContractCall::new(TARGET, "f()", &[], &["bytes".into(), "bytes".into()])?;
    assert!(
        pair.decode_result(&format!("0x{}{}{}", word(64), word(64), word(0)))
            .is_err()
    );
    let array = DeclaredContractCall::new(TARGET, "f()", &[], &["uint256[]".into()])?;
    assert!(
        array
            .decode_result(&format!("0x{}{}", word(32), "ff".repeat(32)))
            .is_err()
    );
    Ok(())
}
#[test]
fn exact_sign_extension_bool_and_fixed_byte_padding_are_required() -> Result<(), ContractError> {
    let call = DeclaredContractCall::new(
        TARGET,
        "f()",
        &[],
        &["int8".into(), "bool".into(), "bytes1".into()],
    )?;
    let values = DynSolValue::Tuple(vec![
        DynSolValue::Int(alloy_primitives::I256::MINUS_ONE, 8),
        DynSolValue::Bool(true),
        DynSolValue::FixedBytes(alloy_primitives::B256::repeat_byte(0), 1),
    ]);
    let encoded = values.abi_encode_params();
    assert_eq!(
        call.decode_result(&super::super::abi::hex_bytes(&encoded))?,
        [json!("-1"), json!(true), json!("0x00")]
    );
    for index in [0, 63, 95] {
        let mut bad = encoded.clone();
        bad[index] = if index == 0 { 0 } else { 2 };
        assert!(
            call.decode_result(&super::super::abi::hex_bytes(&bad))
                .is_err()
        );
    }
    // Exercise a valid empty dynamic array without accepting an empty tuple array.
    assert!(types::parse("()[]").is_err());
    let ty = DynSolType::parse("string[]").map_err(|_| ContractError::UnsupportedAbi)?;
    let data = DynSolValue::Tuple(vec![DynSolValue::Array(vec![])]).abi_encode_params();
    let empty = DeclaredContractCall::new(TARGET, "f()", &[], &[ty.sol_type_name().into_owned()])?;
    assert_eq!(
        empty.decode_result(&super::super::abi::hex_bytes(&data))?,
        [json!([])]
    );
    Ok(())
}

#[test]
fn address_arguments_retain_checksum_checks_at_every_nesting_level() {
    let valid = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
    for value in [
        valid.to_owned(),
        valid.to_ascii_lowercase(),
        format!("0x{}", valid[2..].to_ascii_uppercase()),
    ] {
        assert!(
            DeclaredContractCall::new(TARGET, "f((address[])[])", &[json!([[[value]]])], &[])
                .is_ok()
        );
    }
    let invalid = "0x9858EfFd232B4033E47D90003D41EC34EcaEda94";
    assert!(DeclaredContractCall::new(TARGET, "f(address)", &[json!(invalid)], &[]).is_err());
    assert!(
        DeclaredContractCall::new(TARGET, "f((address[])[])", &[json!([[[invalid]]])], &[])
            .is_err()
    );
}
