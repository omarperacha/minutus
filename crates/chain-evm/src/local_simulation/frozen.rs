//! Proof-checked Base state is frozen before signed authority exists.
use super::{ProvedState, SimulationFeeFacts, context};
use crate::{
    EvmRpcUrl,
    contract::{ContractBlock, UnsignedContractIntent},
};
use evm_simulation::offline::{FrozenBaseRequest, base_native_manifest};
use evm_simulation::{SimulationControl, SimulationError, SimulationRequest, StateQuery};

pub(crate) struct ProvedBaseSnapshot {
    request: SimulationRequest,
    state: ProvedState,
    frozen: FrozenBaseRequest,
}
impl ProvedBaseSnapshot {
    pub(crate) const fn request(&self) -> &SimulationRequest {
        &self.request
    }
    pub(crate) const fn state(&self) -> &ProvedState {
        &self.state
    }
    pub(crate) const fn frozen(&self) -> &FrozenBaseRequest {
        &self.frozen
    }
    pub(crate) fn into_frozen(self) -> FrozenBaseRequest {
        self.frozen
    }
}
pub(crate) fn freeze_proved_base(
    endpoint: &EvmRpcUrl,
    intent: &UnsignedContractIntent,
    block: &ContractBlock,
    fees: SimulationFeeFacts,
    slots: &[StateQuery],
    control: &SimulationControl<'_>,
) -> Result<ProvedBaseSnapshot, SimulationError> {
    let (request, mut source) = context::prepare(endpoint, intent, block, fees, control)?;
    let mut manifest = base_native_manifest(&request)?;
    manifest.extend_from_slice(slots);
    let frozen = FrozenBaseRequest::prepare(request.clone(), &manifest, &mut source, control)?;
    let _ = frozen.fee_ceiling()?;
    source.canonical(control)?;
    Ok(ProvedBaseSnapshot {
        request,
        state: source.into_proved_state(),
        frozen,
    })
}
