//! Engine adapters share only authority-free protocol values, not Revm types.
use crate::{
    admission::{self, Rules},
    contract::{SimulationError, SimulationObservation, SimulationRequest},
    protocol::StatePipe,
};
use std::io::{Read, Write};

mod adapter;
pub(crate) mod base;
mod ethereum;

pub(crate) fn execute<R: Read, W: Write>(
    request: &SimulationRequest,
    pipe: StatePipe<R, W>,
) -> Result<SimulationObservation, SimulationError> {
    match admission::validate(request)? {
        Rules::Osaka | Rules::Amsterdam => ethereum::execute(request, pipe),
        // Native Base simulation is retained as an explicit unpassed fee/context
        // gate: no unsigned-call shortcut may silently skip L1/operator charges.
        Rules::BaseCobalt | Rules::BaseDenim => base::execute(request, pipe),
    }
}
