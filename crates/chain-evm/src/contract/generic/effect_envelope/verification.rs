use super::{GenericEffectEnvelope, GenericEffectOperation, GenericError};
use crate::contract::{UnsignedContractIntent, generic::GenericEffects};
use crate::{
    contract::generic::{GenericContractIdentity, layout},
    local_simulation::ProvedState,
};
use alloy_primitives::{Address, U256};
use std::collections::BTreeMap;

impl GenericEffectEnvelope {
    pub(in crate::contract::generic) fn verify(
        &self,
        intent: &UnsignedContractIntent,
        effects: &GenericEffects,
        identity: &GenericContractIdentity,
        state: &ProvedState,
    ) -> Result<(), GenericError> {
        self.verify_relations(intent, effects, identity, state)?;
        for word in effects.storage_changes() {
            if word.storage_address != intent.call().target() {
                return Err(GenericError::OpaqueEffects);
            }
            for field in &word.fields {
                let path = if field.storage_component {
                    field
                        .path
                        .get(
                            ..field
                                .path
                                .len()
                                .checked_sub(2)
                                .ok_or(GenericError::OpaqueEffects)?,
                        )
                        .ok_or(GenericError::OpaqueEffects)?
                } else {
                    &field.path
                };
                let relation = self
                    .relations
                    .iter()
                    .find(|relation| relation.path == path)
                    .ok_or(GenericError::OpaqueEffects)?;
                if field.storage_component {
                    continue;
                }
                let expected = match relation.operation {
                    GenericEffectOperation::Set => relation.value.clone(),
                    GenericEffectOperation::Increase | GenericEffectOperation::Decrease => {
                        let before = number(&field.before)?;
                        let amount = number(&relation.value)?;
                        let after = if relation.operation == GenericEffectOperation::Increase {
                            before.checked_add(amount)
                        } else {
                            before.checked_sub(amount)
                        }
                        .ok_or(GenericError::OpaqueEffects)?;
                        serde_json::json!(after.to_string())
                    }
                };
                if field.after != expected {
                    return Err(GenericError::OpaqueEffects);
                }
            }
        }
        // This initial grammar has no external business CALL/STATICCALL or
        // CALLCODE. Exact minimal-proxy dispatch is the sole frame exception.
        for (index, frame) in effects.frames().iter().enumerate() {
            if (index == 0 && frame.kind != "call")
                || (index > 0 && frame.kind != "delegateCall")
                || frame.storage_address != intent.call().target()
            {
                return Err(GenericError::OpaqueEffects);
            }
        }
        let value = U256::from_str_radix(intent.value_wei(), 10)
            .map_err(|_| GenericError::OpaqueEffects)?;
        let transfers = effects.native_transfers();
        if value == U256::ZERO {
            if !transfers.is_empty() {
                return Err(GenericError::OpaqueEffects);
            }
        } else if transfers.len() != 1
            || transfers[0].from != intent.account()
            || transfers[0].to != intent.call().target()
            || transfers[0].value_wei != value.to_string()
        {
            return Err(GenericError::OpaqueEffects);
        }
        Ok(())
    }
    fn verify_relations(
        &self,
        intent: &UnsignedContractIntent,
        effects: &GenericEffects,
        identity: &GenericContractIdentity,
        state: &ProvedState,
    ) -> Result<(), GenericError> {
        let address: Address = intent
            .call()
            .target()
            .parse()
            .map_err(|_| GenericError::OpaqueEffects)?;
        let layout = identity
            .storage_artifact(address)
            .ok_or(GenericError::SourceMismatch)?
            .storage_layout();
        let empty = BTreeMap::new();
        let mut after = BTreeMap::new();
        for word in effects.storage_changes() {
            if word.storage_address != intent.call().target() {
                return Err(GenericError::OpaqueEffects);
            }
            let slot = U256::from_str_radix(word.slot.trim_start_matches("0x"), 16)
                .map_err(|_| GenericError::OpaqueEffects)?;
            let value = U256::from_str_radix(word.after.trim_start_matches("0x"), 16)
                .map_err(|_| GenericError::OpaqueEffects)?;
            after.insert(slot, value);
        }
        for relation in &self.relations {
            let before = layout::read_path_after(layout, state, address, &relation.path, &empty)?;
            let observed = layout::read_path_after(layout, state, address, &relation.path, &after)?;
            let expected = match relation.operation {
                GenericEffectOperation::Set => relation.value.clone(),
                GenericEffectOperation::Increase | GenericEffectOperation::Decrease => {
                    let before = number(&before.value)?;
                    let amount = number(&relation.value)?;
                    let value = if relation.operation == GenericEffectOperation::Increase {
                        before.checked_add(amount)
                    } else {
                        before.checked_sub(amount)
                    }
                    .ok_or(GenericError::OpaqueEffects)?;
                    serde_json::json!(value.to_string())
                }
            };
            if observed.value != expected {
                return Err(GenericError::OpaqueEffects);
            }
        }
        Ok(())
    }
}
fn number(value: &serde_json::Value) -> Result<U256, GenericError> {
    U256::from_str_radix(value.as_str().ok_or(GenericError::OpaqueEffects)?, 10)
        .map_err(|_| GenericError::OpaqueEffects)
}
