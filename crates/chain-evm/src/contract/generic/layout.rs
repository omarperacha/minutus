//! Bounded structural Solidity storage interpretation. Labels are source content.
use super::{blob, error::GenericError};
use crate::local_simulation::ProvedState;
use alloy_primitives::{Address, I256, U256, keccak256};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MAX_FIELDS: usize = 128;
const MAX_ITEMS: usize = 32;
const MAX_DEPTH: usize = 8;

/// Canonical source-declared field value; names confer no economic semantics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypedStorageValue {
    /// Variable/member names and canonical collection keys or indices.
    pub path: Vec<String>,
    /// Compiler-declared Solidity type.
    pub solidity_type: String,
    /// Canonical primitive value; integers are exact decimal strings.
    pub value: Value,
}

/// One fully covered source-declared primitive change within a storage word.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypedFieldChange {
    /// Source-declared path, never a trusted authority/token label.
    pub path: Vec<String>,
    /// Exact supported primitive type.
    pub solidity_type: String,
    /// Canonical pre-execution value.
    pub before: Value,
    /// Canonical post-execution value.
    pub after: Value,
    /// True only for an explicitly labelled physical word component of one
    /// dynamic bytes/string field; the logical field is rendered separately.
    pub storage_component: bool,
}

#[derive(Clone)]
struct Leaf {
    path: Vec<String>,
    ty: String,
    slot: U256,
    offset: usize,
    bytes: usize,
}

struct Walker<'a> {
    layout: &'a Value,
    state: &'a ProvedState,
    address: Address,
    after: &'a BTreeMap<U256, U256>,
    preimages: &'a [Vec<u8>],
    leaves: Vec<Leaf>,
    seen: BTreeSet<(U256, usize)>,
    nodes: usize,
    blobs: BTreeMap<U256, TypedFieldChange>,
}

pub(super) fn changes(
    layout: &Value,
    state: &ProvedState,
    address: Address,
    after: &BTreeMap<U256, U256>,
    preimages: &[Vec<u8>],
) -> Result<BTreeMap<U256, Vec<TypedFieldChange>>, GenericError> {
    let mut walker = Walker::new(layout, state, address, after, preimages)?;
    walker.roots()?;
    let mut result = BTreeMap::new();
    for (slot, changed) in after {
        let before = state
            .slot(address, *slot)
            .ok_or(GenericError::MissingState)?;
        let mut covered = U256::ZERO;
        let mut fields = Vec::new();
        for leaf in walker.leaves.iter().filter(|leaf| leaf.slot == *slot) {
            let mask = mask(leaf.bytes)? << (leaf.offset * 8);
            if covered & mask != U256::ZERO {
                return Err(GenericError::OpaqueEffects);
            }
            covered |= mask;
            let old = primitive(&leaf.ty, leaf.bytes, (before & mask) >> (leaf.offset * 8))?;
            let new = primitive(&leaf.ty, leaf.bytes, (*changed & mask) >> (leaf.offset * 8))?;
            if old != new {
                fields.push(TypedFieldChange {
                    path: leaf.path.clone(),
                    solidity_type: leaf.ty.clone(),
                    before: old,
                    after: new,
                    storage_component: false,
                });
            }
        }
        if let Some(blob) = walker.blobs.get(slot) {
            if covered != U256::ZERO {
                return Err(GenericError::OpaqueEffects);
            }
            covered = U256::MAX;
            fields.push(blob.clone());
        }
        if (before ^ *changed) & !covered != U256::ZERO || (before != *changed && fields.is_empty())
        {
            return Err(GenericError::OpaqueEffects);
        }
        if before != *changed {
            result.insert(*slot, fields);
        }
    }
    Ok(result)
}

/// Resolve one explicit source-field path using proved current storage only.
#[cfg(test)]
pub(crate) fn read_path(
    layout: &Value,
    path: &[String],
    state: &ProvedState,
    address: Address,
) -> Result<TypedStorageValue, GenericError> {
    read_condition_path(layout, path, address, |query| {
        let evm_simulation::StateQuery::Storage { slot, .. } = query else {
            return Err(GenericError::OpaqueEffects);
        };
        let slot = U256::from_str_radix(slot.trim_start_matches("0x"), 16)
            .map_err(|_| GenericError::OpaqueEffects)?;
        state.slot(address, slot).ok_or(GenericError::MissingState)
    })
}

/// Static path/type admission does not require a future dynamic-array length or
/// assert that a condition already holds. Actual reads check the proved length.
pub(crate) fn field_type(layout: &Value, path: &[String]) -> Result<String, GenericError> {
    Ok(resolve_path(layout, path, |_| Ok(U256::from(MAX_ITEMS)), |_| {})?.ty)
}

/// Derives hashes for a sealed source-envelope path. This data helper cannot
/// prove a guessed worker key or construct any source/effect certificate.
pub(super) fn path_preimages(
    layout: &Value,
    path: &[String],
) -> Result<Vec<Vec<u8>>, GenericError> {
    let mut preimages = Vec::new();
    resolve_path(
        layout,
        path,
        |_| Ok(U256::from(MAX_ITEMS)),
        |input| preimages.push(input),
    )?;
    Ok(preimages)
}

/// Bounded deterministic query planning. The caller owns proof verification;
/// this function returns interpretation data and cannot mint a certificate.
pub(crate) fn read_condition_path(
    layout: &Value,
    path: &[String],
    address: Address,
    mut query: impl FnMut(evm_simulation::StateQuery) -> Result<U256, GenericError>,
) -> Result<TypedStorageValue, GenericError> {
    let request = |slot: U256| evm_simulation::StateQuery::Storage {
        address: format!("{address:#x}"),
        slot: format!("{slot:#066x}"),
    };
    let leaf = resolve_path(layout, path, |slot| query(request(slot)), |_| {})?;
    if matches!(leaf.ty.as_str(), "bytes" | "string") {
        let value = blob::read(&leaf.ty, leaf.slot, |slot| query(request(slot)))?;
        return Ok(TypedStorageValue {
            path: leaf.path,
            solidity_type: leaf.ty,
            value,
        });
    }
    let word = query(request(leaf.slot))?;
    Ok(TypedStorageValue {
        path: leaf.path,
        solidity_type: leaf.ty.clone(),
        value: primitive(
            &leaf.ty,
            leaf.bytes,
            (word >> (leaf.offset * 8)) & mask(leaf.bytes)?,
        )?,
    })
}

/// Typed prediction over the immutable proved before-state and trusted replay
/// writes. This overlays words without manufacturing another proved snapshot.
pub(crate) fn read_path_after(
    layout: &Value,
    state: &ProvedState,
    address: Address,
    path: &[String],
    after: &BTreeMap<U256, U256>,
) -> Result<TypedStorageValue, GenericError> {
    read_condition_path(layout, path, address, |query| {
        let evm_simulation::StateQuery::Storage { slot, .. } = query else {
            return Err(GenericError::OpaqueEffects);
        };
        let slot = U256::from_str_radix(slot.trim_start_matches("0x"), 16)
            .map_err(|_| GenericError::OpaqueEffects)?;
        after
            .get(&slot)
            .copied()
            .or_else(|| state.slot(address, slot))
            .ok_or(GenericError::MissingState)
    })
}

fn resolve_path(
    layout: &Value,
    path: &[String],
    mut length: impl FnMut(U256) -> Result<U256, GenericError>,
    mut preimage: impl FnMut(Vec<u8>),
) -> Result<Leaf, GenericError> {
    if path.is_empty() || path.len() > MAX_DEPTH || path.iter().any(|v| v.len() > 256) {
        return Err(GenericError::Limit);
    }
    if layout["types"].as_object().is_none_or(|v| v.len() > 4096) {
        return Err(GenericError::Limit);
    }
    let get = |ty: &str| layout["types"].get(ty).ok_or(GenericError::OpaqueEffects);
    let root = named_field(roots(layout)?, &path[0])?;
    let mut slot = decimal(&root["slot"])?;
    let mut offset = number(&root["offset"])?;
    let mut ty = text(&root["type"])?;
    for component in &path[1..] {
        let spec = get(ty)?;
        match spec["encoding"].as_str() {
            Some("mapping") => {
                let key = get(text(&spec["key"])?)?;
                let encoded = encode_key(key, component)?;
                let mut input = Vec::from(encoded);
                input.extend_from_slice(&slot.to_be_bytes::<32>());
                preimage(input.clone());
                slot = U256::from_be_bytes(keccak256(input).0);
                offset = 0;
                ty = text(&spec["value"])?;
            }
            Some("inplace") if spec["members"].is_array() => {
                let member = named_field(
                    spec["members"]
                        .as_array()
                        .ok_or(GenericError::OpaqueEffects)?,
                    component,
                )?;
                slot = slot
                    .checked_add(decimal(&member["slot"])?)
                    .ok_or(GenericError::Limit)?;
                offset = number(&member["offset"])?;
                ty = text(&member["type"])?;
            }
            Some("inplace" | "dynamic_array") if spec["base"].is_string() => {
                let index = index(component)?;
                let length = if spec["encoding"] == "dynamic_array" {
                    let length = length(slot)?;
                    slot = U256::from_be_bytes(keccak256(slot.to_be_bytes::<32>()).0);
                    bounded(length)?
                } else {
                    fixed_length(spec)?
                };
                if index >= length {
                    return Err(GenericError::OpaqueEffects);
                }
                ty = text(&spec["base"])?;
                let (extra, byte) = element(get(ty)?, index)?;
                slot = slot.checked_add(extra).ok_or(GenericError::Limit)?;
                offset = byte;
            }
            _ => return Err(GenericError::OpaqueEffects),
        }
    }
    let spec = get(ty)?;
    let bytes = number(&spec["numberOfBytes"])?;
    let label = text(&spec["label"])?;
    if spec["encoding"] == "bytes"
        && matches!(label, "bytes" | "string")
        && bytes == 32
        && offset == 0
    {
        return Ok(Leaf {
            path: path.into(),
            ty: label.into(),
            slot,
            offset,
            bytes,
        });
    }
    if spec["encoding"] != "inplace"
        || bytes > 32
        || offset > 31
        || bytes > 32 - offset
        || spec["members"].is_array()
        || spec["base"].is_string()
    {
        return Err(GenericError::OpaqueEffects);
    }
    primitive(label, bytes, U256::ZERO)?;
    Ok(Leaf {
        path: path.into(),
        ty: label.into(),
        slot,
        offset,
        bytes,
    })
}

impl<'a> Walker<'a> {
    fn new(
        layout: &'a Value,
        state: &'a ProvedState,
        address: Address,
        after: &'a BTreeMap<U256, U256>,
        preimages: &'a [Vec<u8>],
    ) -> Result<Self, GenericError> {
        if layout["types"].as_object().is_none_or(|v| v.len() > 4096)
            || preimages.len() > 256
            || preimages.iter().any(|v| v.len() > 1024)
        {
            return Err(GenericError::Limit);
        }
        Ok(Self {
            layout,
            state,
            address,
            after,
            preimages,
            leaves: Vec::new(),
            seen: BTreeSet::new(),
            nodes: 0,
            blobs: BTreeMap::new(),
        })
    }
    fn spec(&self, ty: &str) -> Result<&'a Value, GenericError> {
        self.layout["types"]
            .get(ty)
            .ok_or(GenericError::OpaqueEffects)
    }
    fn roots(&mut self) -> Result<(), GenericError> {
        let mut names = BTreeSet::new();
        for root in roots(self.layout)? {
            let name = text(&root["label"])?;
            if !names.insert(name) {
                return Err(GenericError::OpaqueEffects);
            }
            self.walk(
                text(&root["type"])?,
                decimal(&root["slot"])?,
                number(&root["offset"])?,
                vec![name.into()],
                0,
            )?;
        }
        Ok(())
    }
    fn walk(
        &mut self,
        ty: &str,
        slot: U256,
        offset: usize,
        path: Vec<String>,
        depth: usize,
    ) -> Result<(), GenericError> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > 512 || self.leaves.len() > MAX_FIELDS {
            return Err(GenericError::Limit);
        }
        let spec = self.spec(ty)?;
        match spec["encoding"].as_str() {
            Some("mapping") => self.mapping(spec, slot, &path, depth),
            Some("inplace") if spec["members"].is_array() => {
                let mut names = BTreeSet::new();
                for member in spec["members"]
                    .as_array()
                    .ok_or(GenericError::OpaqueEffects)?
                {
                    if !names.insert(text(&member["label"])?) {
                        return Err(GenericError::OpaqueEffects);
                    }
                    let mut child = path.clone();
                    child.push(text(&member["label"])?.into());
                    self.walk(
                        text(&member["type"])?,
                        slot.checked_add(decimal(&member["slot"])?)
                            .ok_or(GenericError::Limit)?,
                        number(&member["offset"])?,
                        child,
                        depth + 1,
                    )?;
                }
                Ok(())
            }
            Some("inplace" | "dynamic_array") if spec["base"].is_string() => {
                let dynamic = spec["encoding"] == "dynamic_array";
                let length = if dynamic {
                    // Untouched collections need not be enumerated. A missing
                    // length cannot authorise a write: the final coverage test
                    // rejects every unmapped changed word.
                    let Some(before) = self.state.slot(self.address, slot) else {
                        return Ok(());
                    };
                    let after = self.after.get(&slot).copied().unwrap_or(before);
                    let count = bounded(before.max(after))?;
                    self.leaf(Leaf {
                        path: [path.clone(), vec!["length".into()]].concat(),
                        ty: "uint256".into(),
                        slot,
                        offset: 0,
                        bytes: 32,
                    })?;
                    count
                } else {
                    fixed_length(spec)?
                };
                let base = text(&spec["base"])?;
                let start = if dynamic {
                    U256::from_be_bytes(keccak256(slot.to_be_bytes::<32>()).0)
                } else {
                    slot
                };
                for i in 0..length {
                    let (extra, byte) = self.element(base, i)?;
                    let mut child = path.clone();
                    child.push(i.to_string());
                    self.walk(
                        base,
                        start.checked_add(extra).ok_or(GenericError::Limit)?,
                        byte,
                        child,
                        depth + 1,
                    )?;
                }
                Ok(())
            }
            Some("inplace") => self.leaf(Leaf {
                path,
                ty: text(&spec["label"])?.into(),
                slot,
                offset,
                bytes: number(&spec["numberOfBytes"])?,
            }),
            Some("bytes") => self.blob(spec, slot, &path),
            // Solidity bytes/string special storage and unstructured state are
            // not partially rendered as a guessed primitive.
            _ => Ok(()),
        }
    }
    fn leaf(&mut self, leaf: Leaf) -> Result<(), GenericError> {
        if leaf.bytes == 0
            || leaf.bytes > 32
            || leaf.offset > 31
            || leaf.bytes > 32 - leaf.offset
            || self.leaves.len() >= MAX_FIELDS
            || !self.seen.insert((leaf.slot, leaf.offset))
        {
            return Err(GenericError::OpaqueEffects);
        }
        primitive(&leaf.ty, leaf.bytes, U256::ZERO)?;
        self.leaves.push(leaf);
        Ok(())
    }
    fn blob(&mut self, spec: &Value, slot: U256, path: &[String]) -> Result<(), GenericError> {
        let start = U256::from_be_bytes(keccak256(slot.to_be_bytes::<32>()).0);
        let touched = self.after.contains_key(&slot)
            || (0..32usize).any(|index| {
                start
                    .checked_add(U256::from(index))
                    .is_some_and(|key| self.after.contains_key(&key))
            });
        if !touched {
            return Ok(());
        }
        let before = |key| {
            self.state
                .slot(self.address, key)
                .ok_or(GenericError::MissingState)
        };
        let after = |key| {
            self.after
                .get(&key)
                .copied()
                .map_or_else(|| before(key), Ok)
        };
        let transition = blob::transition(text(&spec["label"])?, path, slot, before, after)?;
        for (slot, field) in transition.changes {
            if self.blobs.insert(slot, field).is_some() {
                return Err(GenericError::OpaqueEffects);
            }
        }
        Ok(())
    }
    fn element(&self, ty: &str, index: usize) -> Result<(U256, usize), GenericError> {
        element(self.spec(ty)?, index)
    }
    fn mapping(
        &mut self,
        spec: &Value,
        slot: U256,
        path: &[String],
        depth: usize,
    ) -> Result<(), GenericError> {
        let key = self.spec(text(&spec["key"])?)?;
        let value = text(&spec["value"])?;
        let mut visited = BTreeSet::new();
        for input in self.preimages {
            if input.len() != 64 || input[32..] != slot.to_be_bytes::<32>() {
                continue;
            }
            let mut word = [0; 32];
            word.copy_from_slice(&input[..32]);
            let display = decode_key(key, word)?;
            let target = U256::from_be_bytes(keccak256(input).0);
            if !visited.insert(target) {
                continue;
            }
            if visited.len() > MAX_ITEMS {
                return Err(GenericError::Limit);
            }
            let mut child = path.to_vec();
            child.push(display);
            self.walk(value, target, 0, child, depth + 1)?;
        }
        Ok(())
    }
}

fn element(spec: &Value, index: usize) -> Result<(U256, usize), GenericError> {
    let bytes = number(&spec["numberOfBytes"])?;
    let simple = spec["encoding"] == "inplace"
        && !spec["members"].is_array()
        && !spec["base"].is_string()
        && bytes <= 32;
    if bytes == 0 {
        return Err(GenericError::OpaqueEffects);
    }
    if simple {
        let per_slot = 32 / bytes;
        Ok((U256::from(index / per_slot), (index % per_slot) * bytes))
    } else {
        Ok((
            U256::from(
                index
                    .checked_mul(bytes.div_ceil(32))
                    .ok_or(GenericError::Limit)?,
            ),
            0,
        ))
    }
}

fn roots(layout: &Value) -> Result<&Vec<Value>, GenericError> {
    layout["storage"]
        .as_array()
        .filter(|v| v.len() <= MAX_FIELDS)
        .ok_or(GenericError::OpaqueEffects)
}
fn named_field<'a>(members: &'a [Value], label: &str) -> Result<&'a Value, GenericError> {
    let mut matches = members
        .iter()
        .filter(|member| member["label"].as_str() == Some(label));
    let field = matches.next().ok_or(GenericError::OpaqueEffects)?;
    if matches.next().is_some() {
        return Err(GenericError::OpaqueEffects);
    }
    Ok(field)
}
fn text(value: &Value) -> Result<&str, GenericError> {
    value
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 512)
        .ok_or(GenericError::OpaqueEffects)
}
fn number(value: &Value) -> Result<usize, GenericError> {
    if let Some(v) = value.as_u64() {
        return usize::try_from(v).map_err(|_| GenericError::Limit);
    }
    text(value)?
        .parse()
        .map_err(|_| GenericError::OpaqueEffects)
}
fn decimal(value: &Value) -> Result<U256, GenericError> {
    U256::from_str_radix(text(value)?, 10).map_err(|_| GenericError::OpaqueEffects)
}
fn index(value: &str) -> Result<usize, GenericError> {
    let n: usize = value.parse().map_err(|_| GenericError::OpaqueEffects)?;
    if n.to_string() != value || n >= MAX_ITEMS {
        return Err(GenericError::Limit);
    }
    Ok(n)
}
fn bounded(value: U256) -> Result<usize, GenericError> {
    let n = usize::try_from(value).map_err(|_| GenericError::Limit)?;
    if n > MAX_ITEMS {
        return Err(GenericError::Limit);
    }
    Ok(n)
}
fn fixed_length(spec: &Value) -> Result<usize, GenericError> {
    let label = text(&spec["label"])?;
    let tail = label
        .rsplit_once('[')
        .and_then(|(_, v)| v.strip_suffix(']'))
        .ok_or(GenericError::OpaqueEffects)?;
    let n: usize = tail.parse().map_err(|_| GenericError::OpaqueEffects)?;
    if n == 0 || n > MAX_ITEMS {
        return Err(GenericError::Limit);
    }
    Ok(n)
}
fn mask(bytes: usize) -> Result<U256, GenericError> {
    match bytes {
        32 => Ok(U256::MAX),
        1..=31 => Ok((U256::ONE << (bytes * 8)) - U256::ONE),
        _ => Err(GenericError::OpaqueEffects),
    }
}

fn primitive(ty: &str, bytes: usize, value: U256) -> Result<Value, GenericError> {
    if let Some(bits) = ty
        .strip_prefix("uint")
        .and_then(|v| v.parse::<usize>().ok())
        && bits == bytes * 8
        && (8..=256).contains(&bits)
    {
        return Ok(json!(value.to_string()));
    }
    if let Some(bits) = ty.strip_prefix("int").and_then(|v| v.parse::<usize>().ok())
        && bits == bytes * 8
        && (8..=256).contains(&bits)
    {
        let full = if value.bit(bits - 1) {
            value | !mask(bytes)?
        } else {
            value
        };
        return Ok(json!(I256::from_raw(full).to_string()));
    }
    match ty {
        "bool" if bytes == 1 && value <= U256::ONE => Ok(json!(value == U256::ONE)),
        "address" if bytes == 20 => Ok(json!(format!(
            "{:#x}",
            Address::from_slice(&value.to_be_bytes::<32>()[12..])
        ))),
        _ => {
            if let Some(size) = ty
                .strip_prefix("bytes")
                .and_then(|v| v.parse::<usize>().ok())
                && size == bytes
                && (1..=32).contains(&size)
            {
                return Ok(json!(format!(
                    "0x{}",
                    alloy_primitives::hex::encode(&value.to_be_bytes::<32>()[32 - size..])
                )));
            }
            Err(GenericError::OpaqueEffects)
        }
    }
}
fn decode_key(spec: &Value, word: [u8; 32]) -> Result<String, GenericError> {
    let ty = text(&spec["label"])?;
    let bytes = number(&spec["numberOfBytes"])?;
    let full = U256::from_be_bytes(word);
    let value = if ty.starts_with("bytes") {
        if bytes == 0 || bytes > 32 || word[bytes..].iter().any(|b| *b != 0) {
            return Err(GenericError::OpaqueEffects);
        }
        U256::from_be_slice(&word[..bytes])
    } else {
        full & mask(bytes)?
    };
    let decoded = primitive(ty, bytes, value)?;
    let display = match decoded {
        Value::String(s) => s,
        Value::Bool(b) => b.to_string(),
        _ => return Err(GenericError::OpaqueEffects),
    };
    if encode_key(spec, &display)? != word {
        return Err(GenericError::OpaqueEffects);
    }
    Ok(display)
}
fn encode_key(spec: &Value, key: &str) -> Result<[u8; 32], GenericError> {
    let ty = text(&spec["label"])?;
    let bytes = number(&spec["numberOfBytes"])?;
    let value = match ty {
        "bool" => match key {
            "true" => U256::ONE,
            "false" => U256::ZERO,
            _ => return Err(GenericError::OpaqueEffects),
        },
        "address" => {
            let addr: Address = key.parse().map_err(|_| GenericError::OpaqueEffects)?;
            if format!("{addr:#x}") != key {
                return Err(GenericError::OpaqueEffects);
            }
            U256::from_be_slice(addr.as_slice())
        }
        _ if ty.starts_with("uint") => {
            U256::from_str_radix(key, 10).map_err(|_| GenericError::OpaqueEffects)?
        }
        _ if ty.starts_with("int") => key
            .parse::<I256>()
            .map_err(|_| GenericError::OpaqueEffects)?
            .into_raw(),
        _ if ty.starts_with("bytes") => {
            let data = alloy_primitives::hex::decode(
                key.strip_prefix("0x").ok_or(GenericError::OpaqueEffects)?,
            )
            .map_err(|_| GenericError::OpaqueEffects)?;
            if data.len() != bytes || bytes > 32 {
                return Err(GenericError::OpaqueEffects);
            }
            let mut word = [0; 32];
            word[..bytes].copy_from_slice(&data);
            return Ok(word);
        }
        _ => return Err(GenericError::OpaqueEffects),
    };
    let canonical = primitive(ty, bytes, value & mask(bytes)?)?;
    let display = match canonical {
        Value::String(s) => s,
        Value::Bool(b) => b.to_string(),
        _ => return Err(GenericError::OpaqueEffects),
    };
    if display != key {
        return Err(GenericError::OpaqueEffects);
    }
    Ok(value.to_be_bytes::<32>())
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
