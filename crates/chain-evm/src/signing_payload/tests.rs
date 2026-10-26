use super::*;

const ACCOUNT: &str = "0x9858effd232b4033e47d90003d41ec34ecaeda94";
const RECIPIENT: &str = "0x1111111111111111111111111111111111111111";
const BASE_USDC: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";

fn from_hex(value: &str) -> Result<Vec<u8>, SigningPayloadDecodeError> {
    decode_hex(value).ok_or(SigningPayloadDecodeError::MalformedRlp)
}

#[derive(Clone, Copy)]
struct ManualTransaction<'a> {
    chain: &'a [u8],
    nonce: &'a [u8],
    priority: &'a [u8],
    max_fee: &'a [u8],
    gas: &'a [u8],
    destination: &'a [u8],
    value: &'a [u8],
    data: &'a [u8],
}

fn manual_type_two(transaction: ManualTransaction<'_>) -> Vec<u8> {
    let mut fields = Vec::new();
    for integer in [
        transaction.chain,
        transaction.nonce,
        transaction.priority,
        transaction.max_fee,
        transaction.gas,
    ] {
        encode_bytes(integer, &mut fields);
    }
    encode_bytes(transaction.destination, &mut fields);
    encode_bytes(transaction.value, &mut fields);
    encode_bytes(transaction.data, &mut fields);
    encode_list(&[], &mut fields);
    let mut payload = vec![2];
    encode_list(&fields, &mut payload);
    payload
}

#[test]
fn hard_coded_native_fixture_decodes_payload_facts_not_explanation()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = from_hex(concat!(
        "0x02ef0180843b9aca00847735940082520894",
        "1111111111111111111111111111111111111111",
        "880de0b6b3a764000080c0"
    ))?;
    let misleading_model_explanation = "Send 2 USDC to a safe address with no fee";
    let decoded = decode_signing_payload(SigningPayload::Eip1559Transaction {
        account: ACCOUNT,
        payload: &payload,
    })?;
    let DecodedSigningPayload::Eip1559Transaction(decoded) = decoded else {
        return Err("transaction payload decoded as typed data".into());
    };

    assert_eq!(decoded.network(), EvmNetwork::EthereumMainnet);
    assert_eq!(decoded.chain_id(), 1);
    assert_eq!(decoded.destination(), RECIPIENT);
    assert_eq!(decoded.maximum_execution_fee_wei(), "42000000000000");
    assert_eq!(decoded.status(), DecodedTransactionStatus::Verified);
    assert!(matches!(
        decoded.action(),
        DecodedTransactionAction::NativeTransfer {
            value_wei,
            amount_eth,
            ..
        } if value_wei == "1000000000000000000" && amount_eth == "1"
    ));
    assert!(!decoded.raw_payload().contains(misleading_model_explanation));
    Ok(())
}

#[test]
fn configured_token_transfer_is_derived_from_destination_and_calldata()
-> Result<(), Box<dyn std::error::Error>> {
    let recipient = from_hex("0xde709f2102306220921060314715629080e2fb77")?;
    let mut data = TRANSFER_SELECTOR.to_vec();
    data.extend_from_slice(&[0; 12]);
    data.extend_from_slice(&recipient);
    data.extend_from_slice(&[0; 28]);
    data.extend_from_slice(&1_250_000_u32.to_be_bytes());
    let token = from_hex(BASE_USDC)?;
    let payload = manual_type_two(ManualTransaction {
        chain: &[0x21, 0x05],
        nonce: &[1],
        priority: &[1],
        max_fee: &[2],
        gas: &[0xea, 0x60],
        destination: &token,
        value: &[],
        data: &data,
    });
    let decoded = decode_eip1559(ACCOUNT, &payload)?;
    assert_eq!(decoded.network(), EvmNetwork::BaseMainnet);
    assert!(matches!(
        decoded.action(),
        DecodedTransactionAction::TokenTransfer { transfer }
            if transfer.symbol() == "USDC"
                && transfer.amount() == "1.25"
                && transfer.raw_amount() == "1250000"
    ));
    Ok(())
}

#[test]
fn approval_and_typed_data_reuse_typed_authorization_decoding()
-> Result<(), Box<dyn std::error::Error>> {
    let spender = from_hex("0xde709f2102306220921060314715629080e2fb77")?;
    let mut approval_data = APPROVE_SELECTOR.to_vec();
    approval_data.extend_from_slice(&[0; 12]);
    approval_data.extend_from_slice(&spender);
    approval_data.extend_from_slice(&[0; 28]);
    approval_data.extend_from_slice(&1_250_000_u32.to_be_bytes());
    let token = from_hex(BASE_USDC)?;
    let payload = manual_type_two(ManualTransaction {
        chain: &[0x21, 0x05],
        nonce: &[],
        priority: &[1],
        max_fee: &[2],
        gas: &[0xea, 0x60],
        destination: &token,
        value: &[],
        data: &approval_data,
    });
    let decoded = decode_eip1559(ACCOUNT, &payload)?;
    assert_eq!(decoded.status(), DecodedTransactionStatus::Warning);
    assert!(matches!(
        decoded.action(),
        DecodedTransactionAction::TokenApproval { approval }
            if approval.kind() == AuthorizationKind::Approval
                && approval.fields().iter().any(|field| {
                    field.label() == "Allowance" && field.value() == "1.25 USDC"
                })
    ));

    let typed_data = serde_json::json!({
        "types": {
            "EIP712Domain": [{"name":"name","type":"string"},{"name":"version","type":"string"},{"name":"chainId","type":"uint256"},{"name":"verifyingContract","type":"address"}],
            "Permit": [{"name":"owner","type":"address"},{"name":"spender","type":"address"},{"name":"value","type":"uint256"},{"name":"nonce","type":"uint256"},{"name":"deadline","type":"uint256"}]
        },
        "primaryType": "Permit",
        "domain": {"name":"USD Coin","version":"2","chainId":8453,"verifyingContract":BASE_USDC},
        "message": {"owner":ACCOUNT,"spender":"0xde709f2102306220921060314715629080e2fb77","value":"1250000","nonce":"7","deadline":"2000"}
    }).to_string();
    let decoded = decode_signing_payload(SigningPayload::Eip712TypedData {
        network: EvmNetwork::BaseMainnet,
        account: ACCOUNT,
        payload: &typed_data,
        now_unix_seconds: 1_000,
    })?;
    assert!(matches!(
        decoded,
        DecodedSigningPayload::Eip712TypedData(authorization)
            if authorization.kind() == AuthorizationKind::Permit
    ));
    Ok(())
}

#[test]
fn unknown_contract_selector_and_native_value_are_never_verified()
-> Result<(), Box<dyn std::error::Error>> {
    for (destination, value, data, warning) in [
        (
            from_hex("0x2222222222222222222222222222222222222222")?,
            Vec::new(),
            vec![0xde, 0xad, 0xbe, 0xef],
            DecodedTransactionWarningCode::UnknownContract,
        ),
        (
            from_hex(BASE_USDC)?,
            Vec::new(),
            vec![0xde, 0xad, 0xbe, 0xef],
            DecodedTransactionWarningCode::UnknownSelector,
        ),
        (
            from_hex(BASE_USDC)?,
            vec![1],
            TRANSFER_SELECTOR.to_vec(),
            DecodedTransactionWarningCode::UnexpectedNativeValue,
        ),
    ] {
        let payload = manual_type_two(ManualTransaction {
            chain: &[0x21, 0x05],
            nonce: &[],
            priority: &[1],
            max_fee: &[2],
            gas: &[0xea, 0x60],
            destination: &destination,
            value: &value,
            data: &data,
        });
        let decoded = decode_eip1559(ACCOUNT, &payload)?;
        assert_eq!(decoded.status(), DecodedTransactionStatus::Unknown);
        assert_eq!(decoded.warnings()[0].code(), warning);
    }
    Ok(())
}

#[test]
fn malformed_noncanonical_and_unsupported_envelopes_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let valid = from_hex(concat!(
        "0x02ef0180843b9aca00847735940082520894",
        "1111111111111111111111111111111111111111",
        "880de0b6b3a764000080c0"
    ))?;
    assert_eq!(
        decode_eip1559(ACCOUNT, &valid[1..]),
        Err(SigningPayloadDecodeError::UnsupportedTransactionType)
    );
    let mut trailing = valid.clone();
    trailing.push(0);
    assert_eq!(
        decode_eip1559(ACCOUNT, &trailing),
        Err(SigningPayloadDecodeError::MalformedRlp)
    );
    let recipient = from_hex(RECIPIENT)?;
    let noncanonical_zero = manual_type_two(ManualTransaction {
        chain: &[1],
        nonce: &[0],
        priority: &[1],
        max_fee: &[2],
        gas: &[0x52, 0x08],
        destination: &recipient,
        value: &[],
        data: &[],
    });
    assert_eq!(
        decode_eip1559(ACCOUNT, &noncanonical_zero),
        Err(SigningPayloadDecodeError::InvalidInteger)
    );
    Ok(())
}

#[test]
fn one_bit_field_mutation_changes_decoded_facts() -> Result<(), Box<dyn std::error::Error>> {
    let original = from_hex(concat!(
        "0x02ef0180843b9aca00847735940082520894",
        "1111111111111111111111111111111111111111",
        "880de0b6b3a764000080c0"
    ))?;
    let mut changed = original.clone();
    let recipient_byte = changed
        .windows(4)
        .position(|window| window == [0x82, 0x52, 0x08, 0x94])
        .ok_or("recipient prefix missing")?
        + 4;
    changed[recipient_byte] ^= 1;
    let first = decode_eip1559(ACCOUNT, &original)?;
    let second = decode_eip1559(ACCOUNT, &changed)?;
    assert_ne!(first.destination(), second.destination());
    assert_ne!(first.raw_payload(), second.raw_payload());
    Ok(())
}

#[test]
fn canonical_candidate_encoder_round_trips_every_signing_field()
-> Result<(), Box<dyn std::error::Error>> {
    let encoded = encode_candidate(CandidateFields {
        chain_id: 8_453,
        nonce: "7",
        priority_fee: "2000000",
        max_fee: "12000000",
        gas_limit: "21000",
        destination: RECIPIENT,
        value: "1000000000000000000",
        data: "0x",
    })?;
    let decoded = decode_eip1559(ACCOUNT, &encoded)?;
    assert_eq!(decoded.chain_id(), 8_453);
    assert_eq!(decoded.nonce(), "7");
    assert_eq!(decoded.max_priority_fee_per_gas_wei(), "2000000");
    assert_eq!(decoded.max_fee_per_gas_wei(), "12000000");
    assert_eq!(decoded.gas_limit(), "21000");
    assert_eq!(decoded.destination(), RECIPIENT);
    assert_eq!(decoded.value_wei(), "1000000000000000000");
    assert_eq!(decoded.data(), "0x");
    Ok(())
}

#[test]
fn rlp_parser_rejects_long_form_for_short_payloads_and_nonempty_access_lists()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        decode_item(&[0xb8, 0x01, 0x80]),
        Err(SigningPayloadDecodeError::NonCanonicalRlp)
    );
    let mut fields = Vec::new();
    for integer in [&[1][..], &[][..], &[1][..], &[2][..], &[0x52, 0x08][..]] {
        encode_bytes(integer, &mut fields);
    }
    encode_bytes(&from_hex(RECIPIENT)?, &mut fields);
    encode_bytes(&[], &mut fields);
    encode_bytes(&[], &mut fields);
    let mut access = Vec::new();
    encode_list(&[], &mut access);
    encode_list(&access, &mut fields);
    let mut payload = vec![2];
    encode_list(&fields, &mut payload);
    assert_eq!(
        decode_eip1559(ACCOUNT, &payload),
        Err(SigningPayloadDecodeError::UnsupportedAccessList)
    );
    Ok(())
}

#[test]
fn signing_digest_hashes_the_exact_typed_envelope_without_zeta_domain_bytes() {
    assert_eq!(
        hex_prefixed(&eip1559_signing_digest(&[])),
        "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
    );
    let payload = [0x02, 0xc0];
    let mut changed = payload;
    changed[1] ^= 1;
    assert_ne!(
        eip1559_signing_digest(&payload),
        eip1559_signing_digest(&changed)
    );
}

#[test]
fn signed_type_two_assembly_preserves_unsigned_fields_and_adds_canonical_signature()
-> Result<(), Box<dyn std::error::Error>> {
    let unsigned = manual_type_two(ManualTransaction {
        chain: &[1],
        nonce: &[],
        priority: &[1],
        max_fee: &[2],
        gas: &[0x52, 0x08],
        destination: &[0x11; 20],
        value: &[],
        data: &[],
    });
    let signed = assemble_signed_eip1559_transaction(&unsigned, &[0x11; 32], &[0x22; 32], true)?;
    let signed = decode_hex(&signed).ok_or("assembled output must be hex")?;
    assert_eq!(signed[0], 2);
    let (list, consumed) = decode_item(&signed[1..])?;
    assert_eq!(consumed + 1, signed.len());
    let items = list_items(list.payload)?;
    assert_eq!(items.len(), 12);
    assert_eq!(items[9].payload, &[1]);
    assert_eq!(items[10].payload, &[0x11; 32]);
    assert_eq!(items[11].payload, &[0x22; 32]);
    Ok(())
}
