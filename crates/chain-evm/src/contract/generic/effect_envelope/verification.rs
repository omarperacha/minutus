//! Complete owner-qualified snapshot and attempted-write cross-checks.
use super::{GenericEffectEnvelope, GenericEffectOperation, GenericEffectRelation, GenericError};
use crate::contract::{
    UnsignedContractIntent,
    generic::{GenericContractIdentity, GenericEffects, layout},
};
use crate::local_simulation::ProvedState;
use alloy_primitives::{Address, U256};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

type Overlays = BTreeMap<Address, BTreeMap<U256, U256>>;
impl GenericEffectEnvelope {
    pub(in crate::contract::generic) fn verify(
        &self,
        intent: &UnsignedContractIntent,
        effects: &GenericEffects,
        identity: &GenericContractIdentity,
        state: &ProvedState,
        written: &[(Address, U256)],
    ) -> Result<(), GenericError> {
        let after = overlays(effects, state)?;
        self.verify_relations(identity, state, &after, written)?;
        self.verify_fields(effects, identity)?;
        super::frames::verify(intent, &self.invocations, effects.frames(), identity, state)?;
        verify_native(intent, effects.native_transfers())
    }
    fn verify_relations(
        &self,
        identity: &GenericContractIdentity,
        state: &ProvedState,
        after: &Overlays,
        written: &[(Address, U256)],
    ) -> Result<(), GenericError> {
        if self.relations.len() > 32 || written.len() > 128 {
            return Err(GenericError::Limit);
        }
        let mut permitted = BTreeSet::new();
        let empty = BTreeMap::new();
        for (index, relation) in self.relations.iter().enumerate() {
            if self.relations[..index].iter().any(|old| {
                old.storage_address == relation.storage_address
                    && (old.path.starts_with(&relation.path)
                        || relation.path.starts_with(&old.path))
            }) {
                return Err(GenericError::OpaqueEffects);
            }
            let owner: Address = relation
                .storage_address
                .parse()
                .map_err(|_| GenericError::OpaqueEffects)?;
            let (code, artifact) = identity.immutable_dispatch(owner)?;
            if relation.code_address != format!("{code:#x}") || !relation.checked {
                return Err(GenericError::SourceMismatch);
            }
            let overlay = after.get(&owner).unwrap_or(&empty);
            let before = layout::read_path_after(
                artifact.storage_layout(),
                state,
                owner,
                &relation.path,
                &empty,
            )?;
            let observed = layout::read_path_after(
                artifact.storage_layout(),
                state,
                owner,
                &relation.path,
                overlay,
            )?;
            if observed.value != expected(relation, &before.value)? {
                return Err(GenericError::OpaqueEffects);
            }
            for slot in layout::write_slots(
                artifact.storage_layout(),
                state,
                owner,
                &relation.path,
                overlay,
            )? {
                permitted.insert((owner, slot));
            }
            if permitted.len() > 128 {
                return Err(GenericError::Limit);
            }
        }
        if written.iter().any(|key| !permitted.contains(key)) {
            return Err(GenericError::OpaqueEffects);
        }
        Ok(())
    }
    fn verify_fields(
        &self,
        effects: &GenericEffects,
        identity: &GenericContractIdentity,
    ) -> Result<(), GenericError> {
        for word in effects.storage_changes() {
            let owner: Address = word
                .storage_address
                .parse()
                .map_err(|_| GenericError::OpaqueEffects)?;
            let (_, artifact) = identity.immutable_dispatch(owner)?;
            for field in &word.fields {
                let path = if field.storage_component {
                    let count = field
                        .path
                        .len()
                        .checked_sub(2)
                        .ok_or(GenericError::OpaqueEffects)?;
                    if field.path[count] != "$storage"
                        || field.path[count + 1] != word.slot
                        || field.solidity_type != "bytes32"
                        || field.before != serde_json::json!(word.before)
                        || field.after != serde_json::json!(word.after)
                    {
                        return Err(GenericError::OpaqueEffects);
                    }
                    &field.path[..count]
                } else {
                    &field.path
                };
                let relation = self
                    .relations
                    .iter()
                    .find(|r| r.storage_address == word.storage_address && r.path == path)
                    .ok_or(GenericError::OpaqueEffects)?;
                if field.storage_component {
                    if !matches!(
                        layout::field_type(artifact.storage_layout(), path)?.as_str(),
                        "bytes" | "string"
                    ) {
                        return Err(GenericError::OpaqueEffects);
                    }
                } else if field.after != expected(relation, &field.before)? {
                    return Err(GenericError::OpaqueEffects);
                }
            }
        }
        Ok(())
    }
}
fn overlays(effects: &GenericEffects, state: &ProvedState) -> Result<Overlays, GenericError> {
    let mut result = Overlays::new();
    for word in effects.storage_changes() {
        let owner: Address = word
            .storage_address
            .parse()
            .map_err(|_| GenericError::OpaqueEffects)?;
        let slot = word_value(&word.slot)?;
        let before = word_value(&word.before)?;
        let after = word_value(&word.after)?;
        if word.fields.is_empty()
            || before == after
            || state.slot(owner, slot) != Some(before)
            || result
                .entry(owner)
                .or_default()
                .insert(slot, after)
                .is_some()
        {
            return Err(GenericError::OpaqueEffects);
        }
    }
    Ok(result)
}
fn word_value(value: &str) -> Result<U256, GenericError> {
    let text = value
        .strip_prefix("0x")
        .filter(|v| v.len() == 64)
        .ok_or(GenericError::OpaqueEffects)?;
    let result = U256::from_str_radix(text, 16).map_err(|_| GenericError::OpaqueEffects)?;
    if format!("{result:#066x}") != value {
        return Err(GenericError::OpaqueEffects);
    }
    Ok(result)
}
fn expected(relation: &GenericEffectRelation, before: &Value) -> Result<Value, GenericError> {
    if relation.operation == GenericEffectOperation::Set {
        return Ok(relation.value.clone());
    }
    let before = number(before)?;
    let amount = number(&relation.value)?;
    let result = match relation.operation {
        GenericEffectOperation::Increase => before.checked_add(amount),
        GenericEffectOperation::Decrease => before.checked_sub(amount),
        GenericEffectOperation::Set => None,
    }
    .ok_or(GenericError::OpaqueEffects)?;
    Ok(serde_json::json!(result.to_string()))
}
fn number(value: &Value) -> Result<U256, GenericError> {
    U256::from_str_radix(value.as_str().ok_or(GenericError::OpaqueEffects)?, 10)
        .map_err(|_| GenericError::OpaqueEffects)
}
fn verify_native(
    intent: &UnsignedContractIntent,
    transfers: &[crate::contract::generic::GenericNativeTransfer],
) -> Result<(), GenericError> {
    let value =
        U256::from_str_radix(intent.value_wei(), 10).map_err(|_| GenericError::OpaqueEffects)?;
    if value == U256::ZERO {
        return if transfers.is_empty() {
            Ok(())
        } else {
            Err(GenericError::OpaqueEffects)
        };
    }
    if intent.network().is_base()
        || transfers.len() != 1
        || transfers[0].from != intent.account()
        || transfers[0].to != intent.call().target()
        || transfers[0].value_wei != value.to_string()
    {
        return Err(GenericError::OpaqueEffects);
    }
    Ok(())
}
