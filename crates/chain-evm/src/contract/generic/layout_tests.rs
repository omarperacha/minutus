use super::*;
use alloy_primitives::Bytes;
use alloy_trie::TrieAccount;

fn state(slots: &[(U256, U256)]) -> ProvedState {
    ProvedState::test_fixture(
        [(Address::ZERO, (true, TrieAccount::default(), Bytes::new()))].into(),
        slots
            .iter()
            .map(|(k, v)| ((Address::ZERO, *k), *v))
            .collect(),
    )
}
fn layout() -> Value {
    json!({"storage":[{"label":"counter","slot":"0","offset":0,"type":"u"},{"label":"flags","slot":"1","offset":0,"type":"s"},{"label":"items","slot":"3","offset":0,"type":"a"},{"label":"claimedBalance","slot":"5","offset":0,"type":"m"}],"types":{
        "u":{"encoding":"inplace","label":"uint256","numberOfBytes":"32"},
        "small":{"encoding":"inplace","label":"uint128","numberOfBytes":"16"},
        "b":{"encoding":"inplace","label":"bool","numberOfBytes":"1"},
        "s":{"encoding":"inplace","label":"struct Example.Flags","numberOfBytes":"32","members":[{"label":"enabled","slot":"0","offset":0,"type":"b"},{"label":"limit","slot":"0","offset":1,"type":"small"}]},
        "a":{"encoding":"inplace","label":"uint128[3]","numberOfBytes":"64","base":"small"},
        "m":{"encoding":"mapping","label":"mapping(uint256 => uint256)","numberOfBytes":"32","key":"u","value":"u"}
    }})
}

#[test]
fn independently_selected_counter_struct_packed_static_array_and_mapping_are_exact()
-> Result<(), GenericError> {
    let mut key = Vec::from(U256::from(42).to_be_bytes::<32>());
    key.extend_from_slice(&U256::from(5).to_be_bytes::<32>());
    let map = U256::from_be_bytes(keccak256(&key).0);
    let state = state(&[
        (U256::ZERO, U256::from(41)),
        (U256::ONE, U256::ZERO),
        (U256::from(3), U256::ZERO),
        (map, U256::from(7)),
    ]);
    let after = [
        (U256::ZERO, U256::from(42)),
        (U256::ONE, (U256::from(99) << 8) | U256::ONE),
        (U256::from(3), U256::from(12) << 128),
        (map, U256::from(8)),
    ]
    .into();
    let changed = changes(&layout(), &state, Address::ZERO, &after, &[key])?;
    assert_eq!(changed[&U256::ZERO][0].path, ["counter"]);
    assert_eq!(changed[&U256::ZERO][0].before, json!("41"));
    assert_eq!(changed[&U256::ONE].len(), 2);
    assert_eq!(changed[&U256::from(3)][0].path, ["items", "1"]);
    assert_eq!(changed[&map][0].path, ["claimedBalance", "42"]);
    assert_eq!(
        read_path(
            &layout(),
            &["claimedBalance".into(), "42".into()],
            &state,
            Address::ZERO
        )?
        .value,
        json!("7")
    );
    Ok(())
}

#[test]
fn missing_mapping_preimage_unknown_slot_and_packed_padding_changes_are_opaque() {
    let state = state(&[(U256::from(999), U256::ZERO), (U256::ONE, U256::ZERO)]);
    for after in [
        [(U256::from(999), U256::ONE)].into(),
        [(U256::ONE, U256::ONE << 255)].into(),
        [(U256::ONE, U256::from(2))].into(),
    ] {
        assert_eq!(
            changes(&layout(), &state, Address::ZERO, &after, &[]),
            Err(GenericError::OpaqueEffects)
        );
    }
}

#[test]
fn folded_constant_mapping_hash_uses_exact_envelope_path_and_never_guesses_other_keys()
-> Result<(), GenericError> {
    let preimages = path_preimages(&layout(), &["claimedBalance".into(), "7".into()])?;
    assert_eq!(preimages.len(), 1);
    let slot = U256::from_be_bytes(keccak256(&preimages[0]).0);
    let unknown = path_preimages(&layout(), &["claimedBalance".into(), "8".into()])?;
    let unknown = U256::from_be_bytes(keccak256(&unknown[0]).0);
    let s = state(&[(slot, U256::from(3)), (unknown, U256::ZERO)]);
    let writes = [(slot, U256::from(4))].into();
    assert_eq!(
        changes(&layout(), &s, Address::ZERO, &writes, &[]),
        Err(GenericError::OpaqueEffects)
    );
    let decoded = changes(&layout(), &s, Address::ZERO, &writes, &preimages)?;
    assert_eq!(decoded[&slot][0].path, ["claimedBalance", "7"]);
    let mut extra = writes;
    extra.insert(unknown, U256::ONE);
    assert_eq!(
        changes(&layout(), &s, Address::ZERO, &extra, &preimages),
        Err(GenericError::OpaqueEffects)
    );
    Ok(())
}

#[test]
fn ambiguous_field_paths_overlaps_and_noncanonical_collection_paths_reject()
-> Result<(), GenericError> {
    let s = state(&[(U256::ZERO, U256::ZERO), (U256::ONE, U256::ZERO)]);
    let mut duplicate = layout();
    duplicate["storage"][1]["label"] = json!("counter");
    assert_eq!(
        read_path(&duplicate, &["counter".into()], &s, Address::ZERO),
        Err(GenericError::OpaqueEffects)
    );
    assert_eq!(
        changes(
            &duplicate,
            &s,
            Address::ZERO,
            &[(U256::ZERO, U256::ONE)].into(),
            &[]
        ),
        Err(GenericError::OpaqueEffects)
    );
    let mut overlap = layout();
    overlap["types"]["s"]["members"][1]["offset"] = json!(0);
    assert_eq!(
        changes(
            &overlap,
            &s,
            Address::ZERO,
            &[(U256::ONE, U256::ONE)].into(),
            &[]
        ),
        Err(GenericError::OpaqueEffects)
    );
    assert_eq!(
        field_type(&layout(), &["items".into(), "01".into()]),
        Err(GenericError::Limit)
    );
    assert_eq!(
        field_type(&layout(), &["claimedBalance".into(), "01".into()]),
        Err(GenericError::OpaqueEffects)
    );
    assert_eq!(
        field_type(&layout(), &["flags".into(), "enabled".into()])?,
        "bool"
    );
    Ok(())
}

#[test]
fn signed_fixed_bytes_address_and_mapping_key_padding_are_canonical() -> Result<(), GenericError> {
    assert_eq!(primitive("int8", 1, U256::from(255))?, json!("-1"));
    assert_eq!(primitive("bytes2", 2, U256::from(0xabcd))?, json!("0xabcd"));
    assert_eq!(
        primitive("bool", 1, U256::from(2)),
        Err(GenericError::OpaqueEffects)
    );
    for (label, size, key) in [
        ("int8", 1, "-128"),
        ("uint8", 1, "255"),
        ("bytes2", 2, "0xabcd"),
        ("bool", 1, "true"),
        ("address", 20, "0x1111111111111111111111111111111111111111"),
    ] {
        let spec = json!({"label":label,"numberOfBytes":size.to_string()});
        assert_eq!(decode_key(&spec, encode_key(&spec, key)?)?, key);
    }
    let unsigned = json!({"label":"uint8","numberOfBytes":"1"});
    assert!(encode_key(&unsigned, "256").is_err());
    assert!(encode_key(&unsigned, "01").is_err());
    let mut dirty = [0u8; 32];
    dirty[0] = 1;
    assert!(decode_key(&unsigned, dirty).is_err());
    Ok(())
}

#[test]
fn bounded_dynamic_array_updates_resolve_and_oversize_or_missing_length_fail_closed()
-> Result<(), GenericError> {
    let layout = json!({"storage":[{"label":"values","slot":"0","offset":0,"type":"a"}],"types":{"a":{"encoding":"dynamic_array","label":"uint256[]","numberOfBytes":"32","base":"u"},"u":{"encoding":"inplace","label":"uint256","numberOfBytes":"32"}}});
    let slot = U256::from_be_bytes(keccak256([0u8; 32]).0);
    assert_eq!(
        field_type(&layout, &["values".into(), "31".into()])?,
        "uint256"
    );
    assert_eq!(
        field_type(&layout, &["values".into(), "32".into()]),
        Err(GenericError::Limit)
    );
    let s = state(&[(U256::ZERO, U256::from(2)), (slot, U256::from(3))]);
    let result = changes(
        &layout,
        &s,
        Address::ZERO,
        &[(slot, U256::from(4))].into(),
        &[],
    )?;
    assert_eq!(result[&slot][0].path, ["values", "0"]);
    assert_eq!(
        read_path(&layout, &["values".into(), "0".into()], &s, Address::ZERO)?.value,
        json!("3")
    );
    let absent = state(&[(slot, U256::from(3))]);
    assert_eq!(
        changes(
            &layout,
            &absent,
            Address::ZERO,
            &[(slot, U256::from(4))].into(),
            &[]
        ),
        Err(GenericError::OpaqueEffects)
    );
    let big = state(&[(U256::ZERO, U256::from(33)), (slot, U256::from(3))]);
    assert_eq!(
        changes(
            &layout,
            &big,
            Address::ZERO,
            &[(slot, U256::from(4))].into(),
            &[]
        ),
        Err(GenericError::Limit)
    );
    Ok(())
}

#[test]
fn generic_message_and_bytes_setters_cover_all_words_and_source_field_reads()
-> Result<(), GenericError> {
    let layout = json!({"storage":[{"label":"message","slot":"0","offset":0,"type":"s"}],"types":{"s":{"encoding":"bytes","label":"string","numberOfBytes":"32"}}});
    let start = U256::from_be_bytes(keccak256([0u8; 32]).0);
    let before = state(&[(U256::ZERO, U256::ZERO), (start, U256::ZERO)]);
    let data = U256::from_be_bytes([b'a'; 32]);
    let after = [(U256::ZERO, U256::from(65)), (start, data)].into();
    let changed = changes(&layout, &before, Address::ZERO, &after, &[])?;
    assert_eq!(changed.len(), 2);
    assert_eq!(changed[&U256::ZERO][0].solidity_type, "string");
    assert_eq!(changed[&U256::ZERO][0].before, json!(""));
    assert_eq!(changed[&U256::ZERO][0].after, json!("a".repeat(32)));
    assert_eq!(changed[&start][0].path[1], "$storage");
    let current = state(&[(U256::ZERO, U256::from(65)), (start, data)]);
    assert_eq!(
        read_path(&layout, &["message".into()], &current, Address::ZERO)?.value,
        json!("a".repeat(32))
    );
    assert_eq!(field_type(&layout, &["message".into()])?, "string");
    let missing = state(&[(U256::ZERO, U256::ZERO)]);
    assert_eq!(
        changes(&layout, &missing, Address::ZERO, &after, &[]),
        Err(GenericError::MissingState)
    );
    let mut unknown = after;
    unknown.insert(start + U256::from(40), U256::ONE);
    let with_unknown = state(&[
        (U256::ZERO, U256::ZERO),
        (start, U256::ZERO),
        (start + U256::from(40), U256::ZERO),
    ]);
    assert_eq!(
        changes(&layout, &with_unknown, Address::ZERO, &unknown, &[]),
        Err(GenericError::OpaqueEffects)
    );
    let mut bytes = layout;
    bytes["types"]["s"]["label"] = json!("bytes");
    assert_eq!(
        read_path(&bytes, &["message".into()], &current, Address::ZERO)?.value,
        json!(format!("0x{}", alloy_primitives::hex::encode([b'a'; 32])))
    );
    Ok(())
}
