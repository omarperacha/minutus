//! Online authority-free proof exchange. Offline transport never calls this.
use super::{SimulationControl, SimulationStateSource};
use crate::{
    MAX_STATE_BYTES, MAX_STATE_QUERIES, SimulationError, SimulationObservation, SimulationRequest,
};
use crate::{
    admission,
    protocol::{self, HostMessage, WorkerMessage},
};
use std::{io, time::Duration};
pub(super) fn exchange(
    request: &SimulationRequest,
    source: &mut impl SimulationStateSource,
    control: &SimulationControl<'_>,
    mut input: impl io::Write,
    receiver: &std::sync::mpsc::Receiver<Result<Option<WorkerMessage>, SimulationError>>,
    watched: &super::process::WatchedProcess,
) -> Result<SimulationObservation, SimulationError> {
    protocol::write_frame(
        &HostMessage::Start {
            version: 1,
            request: Box::new(request.clone()),
        },
        &mut input,
    )?;
    let mut count = 0;
    let mut total_bytes = 0;
    let observation = loop {
        control.remaining()?;
        watched.check()?;
        let message = match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(value) => value?.ok_or(SimulationError::Protocol)?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err(SimulationError::Protocol),
        };
        match message {
            WorkerMessage::State { sequence, query } => {
                count += 1;
                if count > MAX_STATE_QUERIES || sequence != count {
                    return Err(SimulationError::Limit);
                }
                admission::validate_query(&query, request.block.number)?;
                let value = source.query(&query, control)?;
                control.remaining()?;
                total_bytes +=
                    protocol::write_frame(&HostMessage::State { sequence, value }, &mut input)?;
                if total_bytes > MAX_STATE_BYTES {
                    return Err(SimulationError::Limit);
                }
            }
            WorkerMessage::Finished { result } => break result?,
        }
    };
    if observation.block_hash != request.block.hash
        || observation.request_commitment != protocol::commitment(request)?
        || observation.effects_qualified
    {
        return Err(SimulationError::Protocol);
    }
    drop(input);
    Ok(observation)
}

// Keep io errors local: provider/pipe diagnostics never become model prose.
impl From<io::Error> for SimulationError {
    fn from(_: io::Error) -> Self {
        Self::Protocol
    }
}
