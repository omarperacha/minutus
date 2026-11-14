use super::*;
const TARGET: &str = "0x1111111111111111111111111111111111111111";

#[test]
fn canonical_transfer_selector_and_independent_decode_reject_tampering() -> Result<(), ContractError>
{
    let call = ContractCall::new(
        TARGET,
        "transfer(address,uint256)",
        &[TARGET.into(), "42".into()],
        &[ContractScalar::Bool],
    )?;
    assert!(call.calldata().starts_with("0xa9059cbb"));
    assert_eq!(call.arguments(), &[TARGET, "42"]);
    for bad in [
        format!("{}00", call.calldata()),
        call.calldata()[..10].into(),
        call.calldata().replacen("a9059cbb", "095ea7b3", 1),
        format!("{}{}", &call.calldata()[..74], "f".repeat(64)),
    ] {
        assert_eq!(
            call.verify_calldata(&bad),
            Err(ContractError::MalformedData)
        );
    }
    Ok(())
}

#[test]
fn deceptive_or_unsupported_signatures_and_encodings_fail_closed() {
    for signature in [
        "transfer",
        "transfer(address,uint)",
        "f(string)",
        "f(uint256[])",
        "f((address,bool))",
        "f(address) ",
        "f( address)",
        "f(address))",
        "x\n(address)",
        "1f()",
    ] {
        assert!(
            ContractCall::new(TARGET, signature, &[], &[]).is_err(),
            "{signature}"
        );
    }
    for value in [
        "-1",
        "1e5",
        "01",
        "+1",
        "",
        "115792089237316195423570985008687907853269984665640564039457584007913129639936",
    ] {
        assert!(ContractCall::new(TARGET, "f(uint256)", &[value.into()], &[]).is_err());
    }
}

#[test]
fn outputs_are_strict_and_declared_types_cannot_prove_semantics() -> Result<(), ContractError> {
    let call = ContractCall::new(
        TARGET,
        "f()",
        &[],
        &[ContractScalar::Bool, ContractScalar::Address],
    )?;
    let good = format!("0x{}{}", "0".repeat(64), "0".repeat(64));
    assert_eq!(
        call.decode_result(&good)?,
        vec!["false", "0x0000000000000000000000000000000000000000"]
    );
    assert!(
        call.decode_result(&format!("0x{}2{}", "0".repeat(63), "0".repeat(64)))
            .is_err()
    );
    assert!(
        call.decode_result(&format!("0x{}1{}", "0".repeat(64), "0".repeat(63)))
            .is_err()
    );
    assert!(call.decode_result(&format!("{good}00")).is_err());
    assert!(call.decode_result("0x").is_err());
    assert!(
        call.decode_result(&format!("0x{}", "é".repeat(64)))
            .is_err()
    );
    Ok(())
}

#[test]
fn exact_maximum_scalar_and_parameter_bounds() -> Result<(), ContractError> {
    let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
    let call = ContractCall::new(
        TARGET,
        "f(uint256,bytes32)",
        &[max.into(), format!("0x{}", "ff".repeat(32))],
        &[ContractScalar::Uint256],
    )?;
    assert_eq!(
        call.decode_result(&format!("0x{}", "ff".repeat(32)))?,
        vec![max]
    );
    assert!(ContractCall::new(TARGET, "f()", &[], &[ContractScalar::Bool; 17]).is_err());
    assert!(
        ContractCall::new(
            TARGET,
            &format!("f({})", ["bool"; 17].join(",")),
            &vec!["true".into(); 17],
            &[]
        )
        .is_err()
    );
    Ok(())
}
