//! Minimal canonical RLP support for unsigned EIP-1559 transaction envelopes.

use super::SigningPayloadDecodeError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ItemKind {
    Bytes,
    List,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Item<'a> {
    pub(super) kind: ItemKind,
    pub(super) payload: &'a [u8],
}

pub(super) fn decode_item(input: &[u8]) -> Result<(Item<'_>, usize), SigningPayloadDecodeError> {
    let prefix = *input
        .first()
        .ok_or(SigningPayloadDecodeError::MalformedRlp)?;
    match prefix {
        0x00..=0x7f => Ok((
            Item {
                kind: ItemKind::Bytes,
                payload: &input[..1],
            },
            1,
        )),
        0x80..=0xb7 => {
            let length = usize::from(prefix - 0x80);
            let payload = bounded_payload(input, 1, length)?;
            if length == 1 && payload[0] < 0x80 {
                return Err(SigningPayloadDecodeError::NonCanonicalRlp);
            }
            Ok((
                Item {
                    kind: ItemKind::Bytes,
                    payload,
                },
                1 + length,
            ))
        }
        0xb8..=0xbf => decode_long(input, prefix - 0xb7, ItemKind::Bytes),
        0xc0..=0xf7 => {
            let length = usize::from(prefix - 0xc0);
            Ok((
                Item {
                    kind: ItemKind::List,
                    payload: bounded_payload(input, 1, length)?,
                },
                1 + length,
            ))
        }
        0xf8..=0xff => decode_long(input, prefix - 0xf7, ItemKind::List),
    }
}

fn decode_long(
    input: &[u8],
    length_of_length: u8,
    kind: ItemKind,
) -> Result<(Item<'_>, usize), SigningPayloadDecodeError> {
    let length_of_length = usize::from(length_of_length);
    let length_bytes = bounded_payload(input, 1, length_of_length)?;
    if length_bytes.first() == Some(&0) {
        return Err(SigningPayloadDecodeError::NonCanonicalRlp);
    }
    let length = length_bytes.iter().try_fold(0_usize, |value, byte| {
        value
            .checked_mul(256)
            .and_then(|value| value.checked_add(usize::from(*byte)))
            .ok_or(SigningPayloadDecodeError::MalformedRlp)
    })?;
    if length < 56 {
        return Err(SigningPayloadDecodeError::NonCanonicalRlp);
    }
    let header = 1_usize
        .checked_add(length_of_length)
        .ok_or(SigningPayloadDecodeError::MalformedRlp)?;
    let payload = bounded_payload(input, header, length)?;
    Ok((
        Item { kind, payload },
        header
            .checked_add(length)
            .ok_or(SigningPayloadDecodeError::MalformedRlp)?,
    ))
}

fn bounded_payload(
    input: &[u8],
    offset: usize,
    length: usize,
) -> Result<&[u8], SigningPayloadDecodeError> {
    let end = offset
        .checked_add(length)
        .ok_or(SigningPayloadDecodeError::MalformedRlp)?;
    input
        .get(offset..end)
        .ok_or(SigningPayloadDecodeError::MalformedRlp)
}

pub(super) fn list_items(input: &[u8]) -> Result<Vec<Item<'_>>, SigningPayloadDecodeError> {
    let mut remaining = input;
    let mut items = Vec::with_capacity(12);
    while !remaining.is_empty() {
        if items.len() == 12 {
            return Err(SigningPayloadDecodeError::UnexpectedFieldCount);
        }
        let (item, consumed) = decode_item(remaining)?;
        items.push(item);
        remaining = &remaining[consumed..];
    }
    Ok(items)
}

pub(super) fn encode_bytes(value: &[u8], output: &mut Vec<u8>) {
    if value.len() == 1 && value[0] < 0x80 {
        output.push(value[0]);
    } else {
        encode_header(0x80, 0xb7, value.len(), output);
        output.extend_from_slice(value);
    }
}

pub(super) fn encode_list(value: &[u8], output: &mut Vec<u8>) {
    encode_header(0xc0, 0xf7, value.len(), output);
    output.extend_from_slice(value);
}

fn encode_header(short_base: u8, long_base: u8, length: usize, output: &mut Vec<u8>) {
    if length <= 55 {
        output.push(short_base + u8::try_from(length).unwrap_or_default());
        return;
    }
    let bytes = length.to_be_bytes();
    let first = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len() - 1);
    let significant = &bytes[first..];
    output.push(long_base + u8::try_from(significant.len()).unwrap_or_default());
    output.extend_from_slice(significant);
}
