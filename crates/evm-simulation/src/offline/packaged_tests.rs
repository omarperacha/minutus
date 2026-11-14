//! Genuine packaged transport tests. The runner must be an isolated `AppSandbox`
//! .app with immutable loaded pins and all actual staged role services. No raw
//! trusted constructor or direct-launch fallback is used. State is synthetic;
//! this is transport qualification, not live chain/source or approval evidence.
use super::{FrozenBaseRequest, OfflineMaterialExpectation, replay_tests};
use crate::*;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
type Facts = Vec<(StateQuery, StateValue)>;
struct Source {
    facts: Facts,
    queries: usize,
}
impl SimulationStateSource for Source {
    fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, SimulationError> {
        control.remaining()?;
        self.queries += 1;
        self.facts
            .iter()
            .find(|(key, _)| key == query)
            .map(|(_, value)| value.clone())
            .ok_or(SimulationError::StateUnavailable)
    }
}
fn fixture() -> Result<(SimulationRequest, Facts), Box<dyn std::error::Error>> {
    replay_tests::fixture(
        SimulationNetwork::BaseMainnet,
        "0x602a60005500",
        "0x",
        vec![],
    )
}
fn online(workers: &TrustedSimulationWorkers) -> Result<(), Box<dyn std::error::Error>> {
    let (mut request, facts) = fixture()?;
    request.network = SimulationNetwork::EthereumMainnet;
    request.block.excess_blob_gas = 10_000_000;
    let cancelled = AtomicBool::new(false);
    let mut source = Source { facts, queries: 0 };
    let observation = simulate_with_trusted_worker(
        workers.online(),
        &request,
        &mut source,
        &SimulationControl::start(&cancelled),
    )?;
    assert_eq!(observation.status, ExecutionStatus::Succeeded);
    assert!(!observation.effects_qualified);
    assert_eq!(observation.storage.len(), 1);
    assert!(observation.storage[0].after.ends_with("2a"));
    assert!(source.queries > 0);
    Ok(())
}
#[test]
#[ignore = "requires faithful isolated AppSandbox app and actual pinned XPC services"]
fn packaged_online_repeats_same_role_without_a_local_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    let workers = TrustedSimulationWorkers::packaged()?;
    online(&workers)?;
    online(&workers)
}
#[test]
#[ignore = "requires faithful isolated AppSandbox app and actual pinned XPC services"]
fn packaged_native_base_uses_frozen_state_and_exact_public_envelope()
-> Result<(), Box<dyn std::error::Error>> {
    let workers = TrustedSimulationWorkers::packaged()?;
    for (network, timestamp) in [
        (SimulationNetwork::BaseMainnet, 1_793_300_000),
        (SimulationNetwork::BaseMainnet, 1_793_901_600),
        (SimulationNetwork::BaseSepolia, 1_793_300_000),
    ] {
        let (mut request, facts) = fixture()?;
        request.network = network;
        request.block.timestamp = timestamp;
        let cancelled = AtomicBool::new(false);
        let control = SimulationControl::start(&cancelled);
        let replay = replay_frozen(&request, &facts, &control)?;
        let expected = OfflineMaterialExpectation::from_trusted_replay(&replay)?;
        let signed = replay_tests::signed(&request, 1)?; // Public, never-funded d=1.
        let manifest = facts
            .iter()
            .map(|(query, _)| query.clone())
            .collect::<Vec<_>>();
        let mut source = Source { facts, queries: 0 };
        let snapshot = FrozenBaseRequest::prepare(request, &manifest, &mut source, &control)?;
        let commitment = snapshot.commitment().to_owned();
        let queries = source.queries;
        let validated =
            super::execute_base_offline(workers.offline(), snapshot, &signed, &control)?
                .validate(&expected)?;
        assert_eq!(validated.snapshot_commitment(), commitment);
        assert_eq!(source.queries, queries);
        ensure_worker_runtime_available()?;
    }
    online(&workers)
}
// The cancellation/crash joins below deliberately own the same private codec as
// production. They validate the genuine envelope and complete snapshot first,
// wait for worker Ready, and taint only its authenticated pipe. The closure does
// not mint an OfflineExecution/certificate or bypass the remote lifecycle.
fn after_signed_input(
    workers: &TrustedSimulationWorkers,
    cancelled: &AtomicBool,
    cancel: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use super::transport::{HostMessage, WorkerMessage};
    let (request, facts) =
        replay_tests::fixture(SimulationNetwork::BaseMainnet, "0x5b600056", "0x", vec![])?;
    let control = SimulationControl::start(cancelled);
    let signed = replay_tests::signed(&request, 1)?;
    let snapshot = FrozenBaseRequest::from_replay_facts(&request, &facts)?;
    let _ = super::envelope::Envelope::decode(&snapshot, &signed)?;
    let _ = snapshot.fee_ceiling()?;
    let mut sent = false;
    let result: Result<(), SimulationError> = crate::supervisor::supervise_trusted(
        workers.offline(),
        &control,
        |mut input, receiver, watched| {
            protocol::write_frame(
                &HostMessage::Begin {
                    version: 1,
                    request: Box::new(request),
                    snapshot_commitment: snapshot.commitment.clone(),
                    fact_count: snapshot.facts.len(),
                },
                &mut input,
            )?;
            for (index, fact) in snapshot.facts.iter().enumerate() {
                protocol::write_frame(
                    &HostMessage::Fact {
                        index,
                        fact: fact.clone(),
                    },
                    &mut input,
                )?;
            }
            match receiver.recv_timeout(control.remaining()?) {
                Ok(Ok(Some(WorkerMessage::Ready {
                    snapshot_commitment,
                }))) if snapshot_commitment == snapshot.commitment => {}
                _ => return Err(SimulationError::Protocol),
            }
            protocol::write_frame(
                &HostMessage::Execute {
                    signed: format!("0x{}", alloy_primitives::hex::encode(signed)),
                },
                &mut input,
            )?;
            sent = true;
            if cancel {
                cancelled.store(true, Ordering::Release);
                return Err(SimulationError::Cancelled);
            }
            // The external owner harness may now kill ONLY this private app's
            // authenticated broker. No production PID/control/test hook exists.
            println!("offline-signed-pipe-written");
            loop {
                watched.check()?;
                control.remaining()?;
                std::thread::sleep(Duration::from_millis(5));
            }
        },
    );
    assert!(
        sent,
        "the actual signed Execute frame must enter the authenticated worker pipe"
    );
    if cancel {
        assert_eq!(result, Err(SimulationError::Cancelled));
    } else {
        assert_eq!(result, Err(SimulationError::WorkerUnavailable));
    }
    Ok(())
}
#[test]
#[ignore = "requires faithful isolated AppSandbox app and actual pinned XPC services"]
fn cancellation_after_signed_input_requires_actual_reap_before_online_resume()
-> Result<(), Box<dyn std::error::Error>> {
    let workers = TrustedSimulationWorkers::packaged()?;
    after_signed_input(&workers, &AtomicBool::new(false), true)?;
    ensure_worker_runtime_available()?;
    online(&workers)
}
#[test]
#[ignore = "external owner harness must kill only its private broker after the pipe-written marker; separate process"]
fn broker_loss_after_signed_input_quarantines_further_online_work()
-> Result<(), Box<dyn std::error::Error>> {
    let workers = TrustedSimulationWorkers::packaged()?;
    after_signed_input(&workers, &AtomicBool::new(false), false)?;
    assert_eq!(
        ensure_worker_runtime_available(),
        Err(SimulationError::WorkerUnavailable)
    );
    assert!(TrustedSimulationWorkers::packaged().is_err());
    assert!(online(&workers).is_err());
    Ok(())
}
