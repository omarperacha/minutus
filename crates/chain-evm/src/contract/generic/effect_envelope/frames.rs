//! Every maintained replay frame must implement one exact source invocation.
use super::{GenericEffectInvocation, GenericError};
use crate::{
    contract::{
        UnsignedContractIntent,
        generic::{GenericContractIdentity, effects::GenericCallEffect, proxy},
    },
    local_simulation::ProvedState,
};
use alloy_primitives::{Address, U256};

#[cfg(test)]
mod tests;

pub(super) fn verify(
    intent: &UnsignedContractIntent,
    invocations: &[GenericEffectInvocation],
    frames: &[GenericCallEffect],
    identity: &GenericContractIdentity,
    state: &ProvedState,
) -> Result<(), GenericError> {
    let root = invocations.first().ok_or(GenericError::OpaqueEffects)?;
    if root.parent.is_some()
        || root.is_static
        || root.storage_address != intent.call().target()
        || root.caller != intent.account()
        || root.function != intent.call().signature()
        || root.arguments != intent.call().arguments()
        || root.value_wei != intent.value_wei()
        || root.calldata != intent.call().calldata()
    {
        return Err(GenericError::OpaqueEffects);
    }
    match_tree(invocations, frames, |invocation| {
        let owner: Address = invocation
            .storage_address
            .parse()
            .map_err(|_| GenericError::OpaqueEffects)?;
        let (code, _) = identity.immutable_dispatch(owner)?;
        if invocation.code_address != format!("{code:#x}") {
            return Err(GenericError::SourceMismatch);
        }
        if code == owner {
            return Ok(false);
        }
        let runtime = state
            .account(owner)
            .ok_or(GenericError::MissingState)?
            .code();
        if proxy::minimal_target(runtime) != Some(code) {
            return Err(GenericError::UnsupportedProxy);
        }
        Ok(true)
    })
}
fn match_tree(
    invocations: &[GenericEffectInvocation],
    frames: &[GenericCallEffect],
    mut fixed_proxy: impl FnMut(&GenericEffectInvocation) -> Result<bool, GenericError>,
) -> Result<(), GenericError> {
    if invocations.is_empty() || invocations.len() > 32 || frames.len() > 128 {
        return Err(GenericError::Limit);
    }
    let mut active = Vec::new();
    let mut depths: Vec<usize> = Vec::new();
    let mut cursor = 0;
    for (index, invocation) in invocations.iter().enumerate() {
        let depth = if index == 0 {
            if invocation.parent.is_some() || invocation.is_static {
                return Err(GenericError::OpaqueEffects);
            }
            1
        } else {
            let parent = usize::from(invocation.parent.ok_or(GenericError::OpaqueEffects)?);
            let position = active
                .iter()
                .position(|id| *id == parent)
                .ok_or(GenericError::OpaqueEffects)?;
            if invocation.value_wei != "0" {
                return Err(GenericError::OpaqueEffects);
            }
            active.truncate(position + 1);
            depths
                .get(parent)
                .copied()
                .ok_or(GenericError::OpaqueEffects)?
                + 1
        };
        let outer = frames.get(cursor).ok_or(GenericError::OpaqueEffects)?;
        let kind = if invocation.is_static {
            "staticCall"
        } else {
            "call"
        };
        match_frame(outer, invocation, kind, &invocation.storage_address, depth)?;
        cursor += 1;
        let implementation_depth = if fixed_proxy(invocation)? {
            let inner = frames.get(cursor).ok_or(GenericError::OpaqueEffects)?;
            match_frame(
                inner,
                invocation,
                "delegateCall",
                &invocation.code_address,
                depth + 1,
            )?;
            if inner.output != outer.output {
                return Err(GenericError::OpaqueEffects);
            }
            cursor += 1;
            depth + 1
        } else {
            if invocation.storage_address != invocation.code_address {
                return Err(GenericError::UnsupportedProxy);
            }
            depth
        };
        depths.push(implementation_depth);
        active.push(index);
    }
    if cursor != frames.len() {
        return Err(GenericError::OpaqueEffects);
    }
    Ok(())
}
fn match_frame(
    frame: &GenericCallEffect,
    invocation: &GenericEffectInvocation,
    kind: &str,
    code: &str,
    depth: usize,
) -> Result<(), GenericError> {
    let value =
        U256::from_str_radix(&invocation.value_wei, 10).map_err(|_| GenericError::OpaqueEffects)?;
    if depth > 16
        || frame.depth != depth
        || frame.kind != kind
        || frame.storage_address != invocation.storage_address
        || frame.code_address != code
        || frame.caller != invocation.caller
        || frame.apparent_value != format!("{value:#x}")
        || frame.input != invocation.calldata
        || !frame.succeeded
        || !frame.committed
    {
        return Err(GenericError::OpaqueEffects);
    }
    Ok(())
}
