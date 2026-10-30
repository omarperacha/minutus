//! A separate closed protocol: once Execute enters, no state broker exists.
use super::{
    envelope::Envelope,
    result::OfflineExecution,
    snapshot::{Fact, FrozenBaseRequest},
};
use crate::{
    SimulationControl, SimulationError, SimulationObservation, SimulationRequest, protocol,
    supervisor::{self, WatchedChild},
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::Duration,
};
#[derive(Deserialize, Serialize)]
#[serde(tag = "message", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum HostMessage {
    Begin {
        version: u8,
        request: Box<SimulationRequest>,
        snapshot_commitment: String,
        fact_count: usize,
    },
    Fact {
        index: usize,
        fact: Fact,
    },
    Execute {
        signed: String,
    },
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "message", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum WorkerMessage {
    Ready {
        snapshot_commitment: String,
    },
    Finished {
        result: Box<Result<SimulationObservation, SimulationError>>,
    },
}
/// Executes a genuine signed envelope against a previously frozen closed state
/// manifest in the mandatory sandbox. Only the trusted host selects the binary.
/// No state source, callback or network operation is accepted after signature.
/// Raw signed-run output remains opaque until checked against host expectations.
///
/// # Errors
/// Rejects body/signature mismatch, missing state, protocol/process failures and
/// deadline/cancellation. No retry or state-manifest enlargement is performed.
pub fn execute_base_offline(
    worker: &crate::TrustedSimulationWorker,
    snapshot: FrozenBaseRequest,
    signed: &[u8],
    control: &SimulationControl<'_>,
) -> Result<OfflineExecution, SimulationError> {
    worker.require(crate::worker_identity::Role::Offline)?;
    execute(worker.path(), Some(worker), snapshot, signed, control)
}
/// Diagnostic-only process exercise. Discards every result, including its
/// validated receipt; a raw path can never mint trusted qualification authority.
///
/// # Errors
/// Uses the same strict envelope, snapshot, material and process validation.
pub fn diagnose_base_offline(
    program: &Path,
    snapshot: FrozenBaseRequest,
    signed: &[u8],
    expected: &super::OfflineMaterialExpectation,
    control: &SimulationControl<'_>,
) -> Result<(), SimulationError> {
    execute(program, None, snapshot, signed, control)?.validate(expected)?;
    Ok(())
}
pub(super) fn execute(
    program: &Path,
    worker: Option<&crate::TrustedSimulationWorker>,
    snapshot: FrozenBaseRequest,
    signed: &[u8],
    control: &SimulationControl<'_>,
) -> Result<OfflineExecution, SimulationError> {
    control.remaining()?;
    let envelope = Envelope::decode(&snapshot, signed)?;
    // Complete fee and arithmetic qualification before authority enters a child.
    let _ = snapshot.fee_ceiling()?;
    let exchange = |mut input,
                    receiver: &Receiver<Result<Option<WorkerMessage>, SimulationError>>,
                    watched: &WatchedChild| {
        protocol::write_frame(
            &HostMessage::Begin {
                version: 1,
                request: Box::new(snapshot.request.clone()),
                snapshot_commitment: snapshot.commitment.clone(),
                fact_count: snapshot.facts.len(),
            },
            &mut input,
        )?;
        for (index, fact) in snapshot.facts.iter().enumerate() {
            control.remaining()?;
            watched.check()?;
            protocol::write_frame(
                &HostMessage::Fact {
                    index,
                    fact: fact.clone(),
                },
                &mut input,
            )?;
        }
        let WorkerMessage::Ready {
            snapshot_commitment,
        } = receive(receiver, watched, control)?
        else {
            return Err(SimulationError::Protocol);
        };
        if snapshot_commitment != snapshot.commitment {
            return Err(SimulationError::InconsistentState);
        }
        protocol::write_frame(
            &HostMessage::Execute {
                signed: format!("{:#x}", envelope.bytes),
            },
            &mut input,
        )?;
        drop(input);
        let WorkerMessage::Finished { result } = receive(receiver, watched, control)? else {
            return Err(SimulationError::Protocol);
        };
        *result
    };
    let observation = match worker {
        Some(worker) => supervisor::supervise_trusted(worker, control, exchange),
        None => supervisor::supervise(program, control, exchange),
    }?;
    Ok(OfflineExecution {
        observation,
        envelope,
        snapshot,
    })
}
fn receive(
    receiver: &Receiver<Result<Option<WorkerMessage>, SimulationError>>,
    watched: &WatchedChild,
    control: &SimulationControl<'_>,
) -> Result<WorkerMessage, SimulationError> {
    loop {
        control.remaining()?;
        watched.check()?;
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(value) => return value?.ok_or(SimulationError::Protocol),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(SimulationError::Protocol),
        }
    }
}
