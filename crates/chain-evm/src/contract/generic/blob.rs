//! Canonical bounded Solidity bytes/string storage, including cleared tails.
use super::{GenericError, layout::TypedFieldChange};
use alloy_primitives::{U256, keccak256};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const MAX_BYTES: usize = 1024;
pub(super) struct BlobTransition {
    pub(super) changes: BTreeMap<U256, TypedFieldChange>,
}
struct Decoded {
    value: Value,
    chunks: usize,
}

pub(super) fn read(
    ty: &str,
    slot: U256,
    mut query: impl FnMut(U256) -> Result<U256, GenericError>,
) -> Result<Value, GenericError> {
    Ok(decode(ty, slot, &mut query)?.value)
}

pub(super) fn transition(
    ty: &str,
    path: &[String],
    slot: U256,
    mut before: impl FnMut(U256) -> Result<U256, GenericError>,
    mut after: impl FnMut(U256) -> Result<U256, GenericError>,
) -> Result<BlobTransition, GenericError> {
    let old = decode(ty, slot, &mut before)?;
    let new = decode(ty, slot, &mut after)?;
    let mut words = BTreeMap::new();
    words.insert(slot, (before(slot)?, after(slot)?));
    let start = U256::from_be_bytes(keccak256(slot.to_be_bytes::<32>()).0);
    for index in 0..old.chunks.max(new.chunks) {
        let key = start
            .checked_add(U256::from(index))
            .ok_or(GenericError::Limit)?;
        let previous = before(key)?;
        let current = after(key)?;
        if index >= new.chunks && current != U256::ZERO {
            return Err(GenericError::OpaqueEffects);
        }
        words.insert(key, (previous, current));
    }
    let mut changes = BTreeMap::new();
    let mut whole = false;
    for (word, (previous, current)) in words {
        if previous == current {
            continue;
        }
        let field = if whole {
            let mut component = path.to_vec();
            component.push("$storage".into());
            component.push(format!("{word:#066x}"));
            TypedFieldChange {
                path: component,
                solidity_type: "bytes32".into(),
                before: json!(format!("{previous:#066x}")),
                after: json!(format!("{current:#066x}")),
                storage_component: true,
            }
        } else {
            if old.value == new.value {
                return Err(GenericError::OpaqueEffects);
            }
            whole = true;
            TypedFieldChange {
                path: path.into(),
                solidity_type: ty.into(),
                before: old.value.clone(),
                after: new.value.clone(),
                storage_component: false,
            }
        };
        changes.insert(word, field);
    }
    Ok(BlobTransition { changes })
}

fn decode(
    ty: &str,
    slot: U256,
    query: &mut impl FnMut(U256) -> Result<U256, GenericError>,
) -> Result<Decoded, GenericError> {
    if !matches!(ty, "bytes" | "string") {
        return Err(GenericError::OpaqueEffects);
    }
    let root = query(slot)?;
    let word = root.to_be_bytes::<32>();
    let (bytes, chunks) = if word[31] & 1 == 0 {
        let length = usize::from(word[31] / 2);
        if length > 31 || word[length..31].iter().any(|byte| *byte != 0) {
            return Err(GenericError::OpaqueEffects);
        }
        (word[..length].to_vec(), 0)
    } else {
        let length =
            usize::try_from((root - U256::ONE) / U256::from(2)).map_err(|_| GenericError::Limit)?;
        if !(32..=MAX_BYTES).contains(&length) {
            return Err(GenericError::OpaqueEffects);
        }
        let chunks = length.div_ceil(32);
        let start = U256::from_be_bytes(keccak256(slot.to_be_bytes::<32>()).0);
        let mut bytes = Vec::with_capacity(chunks * 32);
        for index in 0..chunks {
            let key = start
                .checked_add(U256::from(index))
                .ok_or(GenericError::Limit)?;
            bytes.extend_from_slice(&query(key)?.to_be_bytes::<32>());
        }
        if bytes[length..].iter().any(|byte| *byte != 0) {
            return Err(GenericError::OpaqueEffects);
        }
        bytes.truncate(length);
        (bytes, chunks)
    };
    let value = if ty == "string" {
        Value::String(String::from_utf8(bytes).map_err(|_| GenericError::OpaqueEffects)?)
    } else {
        json!(format!("0x{}", alloy_primitives::hex::encode(bytes)))
    };
    Ok(Decoded { value, chunks })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn encode(bytes: &[u8]) -> BTreeMap<U256, U256> {
        let mut state = BTreeMap::new();
        if bytes.len() < 32 {
            let mut word = [0; 32];
            word[..bytes.len()].copy_from_slice(bytes);
            word[31] = u8::try_from(bytes.len() * 2).unwrap_or(0);
            state.insert(U256::ZERO, U256::from_be_bytes(word));
        } else {
            state.insert(U256::ZERO, U256::from(bytes.len() * 2 + 1));
            let start = U256::from_be_bytes(keccak256([0u8; 32]).0);
            for (index, chunk) in bytes.chunks(32).enumerate() {
                let mut word = [0; 32];
                word[..chunk.len()].copy_from_slice(chunk);
                state.insert(start + U256::from(index), U256::from_be_bytes(word));
            }
        }
        state
    }
    #[test]
    fn short_long_utf8_and_bytes_decode_exactly_through_1024_bytes() -> Result<(), GenericError> {
        for bytes in [
            vec![],
            vec![b'a'; 31],
            vec![b'a'; 32],
            vec![b'a'; 33],
            vec![b'a'; 1024],
        ] {
            let state = encode(&bytes);
            let query = |slot| state.get(&slot).copied().ok_or(GenericError::MissingState);
            assert_eq!(
                read("string", U256::ZERO, query)?,
                json!(String::from_utf8(bytes.clone()).map_err(|_| GenericError::OpaqueEffects)?)
            );
            assert_eq!(
                read("bytes", U256::ZERO, query)?,
                json!(format!("0x{}", alloy_primitives::hex::encode(bytes)))
            );
        }
        let state = encode("é 🦀".as_bytes());
        assert_eq!(
            read("string", U256::ZERO, |slot| state
                .get(&slot)
                .copied()
                .ok_or(GenericError::MissingState))?,
            json!("é 🦀")
        );
        Ok(())
    }
    #[test]
    fn long_to_short_requires_proved_zeroed_tail_and_every_old_new_word() -> Result<(), GenericError>
    {
        let old = encode(&[b'a'; 65]);
        let mut new = encode(b"ok");
        for slot in old.keys() {
            new.entry(*slot).or_insert(U256::ZERO);
        }
        let changes = transition(
            "string",
            &["message".into()],
            U256::ZERO,
            |slot| old.get(&slot).copied().ok_or(GenericError::MissingState),
            |slot| new.get(&slot).copied().ok_or(GenericError::MissingState),
        )?;
        assert_eq!(changes.changes.len(), 4);
        assert_eq!(
            changes
                .changes
                .values()
                .filter(|value| value.solidity_type == "string")
                .count(),
            1
        );
        let start = U256::from_be_bytes(keccak256([0u8; 32]).0);
        new.insert(start, U256::ONE);
        assert!(
            transition(
                "string",
                &["message".into()],
                U256::ZERO,
                |slot| old.get(&slot).copied().ok_or(GenericError::MissingState),
                |slot| new.get(&slot).copied().ok_or(GenericError::MissingState)
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn invalid_root_lengths_padding_utf8_and_missing_chunks_reject() {
        for root in [U256::from(64), U256::from(3), U256::MAX] {
            assert!(read("bytes", U256::ZERO, |_| Ok(root)).is_err());
        }
        let mut short = encode(b"a");
        short.insert(U256::ZERO, short[&U256::ZERO] | (U256::ONE << 128));
        assert!(
            read("string", U256::ZERO, |slot| short
                .get(&slot)
                .copied()
                .ok_or(GenericError::MissingState))
            .is_err()
        );
        let bad = encode(&[0xff]);
        assert!(
            read("string", U256::ZERO, |slot| bad
                .get(&slot)
                .copied()
                .ok_or(GenericError::MissingState))
            .is_err()
        );
        let mut long = encode(&[b'a'; 33]);
        let end = U256::from_be_bytes(keccak256([0u8; 32]).0) + U256::ONE;
        long.insert(end, long[&end] | U256::ONE);
        assert!(
            read("bytes", U256::ZERO, |slot| long
                .get(&slot)
                .copied()
                .ok_or(GenericError::MissingState))
            .is_err()
        );
        long.remove(&end);
        assert_eq!(
            read("bytes", U256::ZERO, |slot| long
                .get(&slot)
                .copied()
                .ok_or(GenericError::MissingState)),
            Err(GenericError::MissingState)
        );
    }
}
