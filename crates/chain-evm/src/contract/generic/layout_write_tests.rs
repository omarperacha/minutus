//! Static key canonicality and physical-write geometry have no execution authority.
use super::*;
use alloy_primitives::Bytes;
use alloy_trie::TrieAccount;

fn state(words: &[(U256, U256)]) -> ProvedState {
    ProvedState::test_fixture(
        [(Address::ZERO, (true, TrieAccount::default(), Bytes::new()))].into(),
        words
            .iter()
            .map(|(k, v)| ((Address::ZERO, *k), *v))
            .collect(),
    )
}
fn layout() -> Value {
    json!({"storage":[{"label":"values","slot":"0","offset":0,"type":"map"},{"label":"items","slot":"1","offset":0,"type":"array"},{"label":"data","slot":"2","offset":0,"type":"blob"}],"types":{
        "u":{"encoding":"inplace","label":"uint256","numberOfBytes":"32"},
        "key":{"encoding":"inplace","label":"bytes4","numberOfBytes":"4"},
        "map":{"encoding":"mapping","label":"mapping(bytes4=>uint256)","numberOfBytes":"32","key":"key","value":"u"},
        "array":{"encoding":"dynamic_array","label":"uint256[]","numberOfBytes":"32","base":"u"},
        "blob":{"encoding":"bytes","label":"bytes","numberOfBytes":"32"}}})
}
#[test]
fn bytes_mapping_key_aliases_reject_during_static_type_admission() -> Result<(), GenericError> {
    let schema = layout();
    assert_eq!(
        field_type(&schema, &["values".into(), "0xabcdef01".into()])?,
        "uint256"
    );
    for key in [
        "0xABCDEF01",
        "0xabCdef01",
        "0Xabcdef01",
        "0xabcdef1",
        "abcdef01",
    ] {
        assert!(
            field_type(&schema, &["values".into(), key.into()]).is_err(),
            "{key}"
        );
    }
    Ok(())
}
#[test]
fn array_index_aliases_reject_before_snapshot_reads() -> Result<(), GenericError> {
    let schema = layout();
    assert_eq!(
        field_type(&schema, &["items".into(), "1".into()])?,
        "uint256"
    );
    for key in ["01", "+1", "0x1", " 1", "1 ", "-0"] {
        assert!(
            field_type(&schema, &["items".into(), key.into()]).is_err(),
            "{key}"
        );
    }
    Ok(())
}
#[test]
fn array_length_addressing_read_is_never_a_writable_element_word() -> Result<(), GenericError> {
    let schema = layout();
    let length = U256::ONE;
    let start = U256::from_be_bytes(keccak256(length.to_be_bytes::<32>()).0);
    let snapshot = state(&[(length, U256::from(3)), (start + U256::ONE, U256::from(7))]);
    let slots = write_slots(
        &schema,
        &snapshot,
        Address::ZERO,
        &["items".into(), "1".into()],
        &BTreeMap::new(),
    )?;
    assert_eq!(slots, BTreeSet::from([start + U256::ONE]));
    assert!(!slots.contains(&length));
    Ok(())
}
#[test]
fn unchanged_and_cleared_blob_tail_words_remain_covered() -> Result<(), GenericError> {
    let schema = layout();
    let root = U256::from(2);
    let start = U256::from_be_bytes(keccak256(root.to_be_bytes::<32>()).0);
    let snapshot = state(&[
        (root, U256::from(129)),
        (start, U256::MAX),
        (start + U256::ONE, U256::MAX),
    ]);
    let path = ["data".into()];
    let unchanged = write_slots(&schema, &snapshot, Address::ZERO, &path, &BTreeMap::new())?;
    assert_eq!(unchanged, BTreeSet::from([root, start, start + U256::ONE]));
    let after = BTreeMap::from([
        (root, U256::ZERO),
        (start, U256::ZERO),
        (start + U256::ONE, U256::ZERO),
    ]);
    assert_eq!(
        write_slots(&schema, &snapshot, Address::ZERO, &path, &after)?,
        unchanged
    );
    assert!(!unchanged.contains(&(start + U256::from(2))));
    Ok(())
}
