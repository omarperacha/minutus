//! Native full-envelope execution over a map-only database. No broker type or
//! online state protocol is used by this worker.
use super::{
    database::FrozenDb,
    envelope::Envelope,
    fees,
    snapshot::FrozenBaseRequest,
    transport::{HostMessage, WorkerMessage},
};
use crate::engine::base::{BoundedInspector, block, transaction};
use crate::{
    AccountChange, ExecutionStatus, LogObservation, MAX_STATE_QUERIES, SimulationError,
    SimulationObservation, StorageChange, admission, protocol,
};
use base_evm::{BaseTransaction, Builder, DefaultBase};
use revm_base::{
    Context, InspectEvm,
    primitives::{U256, address},
};
use std::io::{Read, Write};

/// Fixed dedicated worker entry point. Applies mandatory process limits before
/// consuming any request. Signed bytes arrive only after the closed state map is
/// validated and acknowledged. All errors are fixed sanitised enum values.
///
/// # Errors
/// Any malformed, unsupported or incomplete input fails the complete run.
pub fn run_offline_worker(
    mut input: impl Read,
    mut output: impl Write,
) -> Result<(), SimulationError> {
    crate::resources::apply()?;
    let HostMessage::Begin {
        version: 1,
        request,
        snapshot_commitment,
        fact_count,
    } = protocol::read_frame(&mut input)?
    else {
        return Err(SimulationError::Protocol);
    };
    if fact_count == 0 || fact_count > MAX_STATE_QUERIES as usize {
        return Err(SimulationError::Limit);
    }
    let mut facts = Vec::new();
    let mut bytes = 0;
    for expected in 0..fact_count {
        let HostMessage::Fact { index, fact } = protocol::read_frame(&mut input)? else {
            return Err(SimulationError::Protocol);
        };
        if index != expected {
            return Err(SimulationError::Protocol);
        }
        bytes += serde_json::to_vec(&fact)
            .map_err(|_| SimulationError::Protocol)?
            .len();
        if bytes > crate::MAX_STATE_BYTES {
            return Err(SimulationError::Limit);
        }
        facts.push(fact);
    }
    let snapshot = FrozenBaseRequest::from_facts(*request, facts)?;
    if snapshot.commitment != snapshot_commitment {
        return Err(SimulationError::InconsistentState);
    }
    let _ = snapshot.fee_ceiling()?;
    protocol::write_frame(
        &WorkerMessage::Ready {
            snapshot_commitment,
        },
        &mut output,
    )?;
    let HostMessage::Execute { signed } = protocol::read_frame(&mut input)? else {
        return Err(SimulationError::Protocol);
    };
    let result = admission::data(&signed, super::MAX_ENVELOPE)
        .and_then(|bytes| Envelope::decode(&snapshot, &bytes))
        .and_then(|envelope| execute(&snapshot, &envelope));
    protocol::write_frame(
        &WorkerMessage::Finished {
            result: Box::new(result),
        },
        &mut output,
    )?;
    Ok(())
}
pub(super) fn execute(
    snapshot: &FrozenBaseRequest,
    envelope: &Envelope,
) -> Result<SimulationObservation, SimulationError> {
    let request = &snapshot.request;
    let spec = fees::spec(snapshot)?;
    let engine = match admission::validate(request)? {
        admission::Rules::BaseCobalt => "base1.5.1/Cobalt",
        admission::Rules::BaseDenim => "base1.5.1/Denim",
        _ => return Err(SimulationError::UnsupportedRules),
    };
    let mut cfg = revm_base::context::CfgEnv::new_with_spec(spec);
    cfg.chain_id = request.network.chain_id();
    cfg.memory_limit = crate::MAX_EVM_MEMORY;
    let before = FrozenDb::new(snapshot)?.accounts;
    let mut inspector = BoundedInspector::default();
    let ctx = Context::base()
        .with_db(FrozenDb::new(snapshot)?)
        .with_block(block(request)?)
        .with_cfg(cfg);
    let admin = match request.network {
        crate::SimulationNetwork::BaseMainnet => {
            address!("ce3a3bee7e72e2a24079f3c0cb3b97740ed425a9")
        }
        crate::SimulationNetwork::BaseSepolia => {
            address!("5f43072722f59964d886cbb507f6a85ca0759d42")
        }
        _ => return Err(SimulationError::UnsupportedRules),
    };
    let mut evm =
        ctx.build_with_inspector_and_activation_admin_address(&mut inspector, Some(admin));
    let mut tx = BaseTransaction::new(transaction(request)?);
    tx.enveloped_tx = Some(envelope.bytes.clone());
    let execution = evm.inspect_tx(tx);
    drop(evm);
    if let Some(fault) = inspector.fault {
        return Err(fault);
    }
    let result = execution.map_err(|error| match error {
        revm_base::context_interface::result::EVMError::Database(error) => error,
        _ => SimulationError::TransactionRejected,
    })?;
    if !result.result.is_success() {
        return Err(SimulationError::TransactionRejected);
    }
    let status = ExecutionStatus::Succeeded;
    let output = result
        .result
        .output()
        .map_or_else(|| "0x".into(), |bytes| format!("{bytes:#x}"));
    if output.len() > 16_386 {
        return Err(SimulationError::Limit);
    }
    observation(
        request,
        engine,
        result,
        &before,
        inspector.calls,
        output,
        status,
    )
}

fn observation(
    request: &crate::SimulationRequest,
    engine: &str,
    result: revm_base::context_interface::result::ResultAndState<base_evm::BaseHaltReason>,
    before: &std::collections::BTreeMap<
        revm_base::primitives::Address,
        Option<revm_base::state::AccountInfo>,
    >,
    calls: Vec<crate::CallObservation>,
    output: String,
    status: ExecutionStatus,
) -> Result<SimulationObservation, SimulationError> {
    let mut logs = Vec::new();
    let mut log_bytes = 0;
    if status == ExecutionStatus::Succeeded {
        for log in result.result.logs() {
            log_bytes += log.data.data.len();
            if logs.len() >= 64 || log_bytes > 16 * 1024 {
                return Err(SimulationError::Limit);
            }
            logs.push(LogObservation {
                address: format!("{:#x}", log.address),
                topics: log
                    .data
                    .topics()
                    .iter()
                    .map(|topic| format!("{topic:#x}"))
                    .collect(),
                data: format!("{:#x}", log.data.data),
            });
        }
    }
    let mut accounts = Vec::new();
    let mut storage = Vec::new();
    for (address, account) in result.state {
        if account.is_selfdestructed() || account.is_created() {
            return Err(SimulationError::UnsupportedExecution);
        }
        if account.is_touched() {
            let original = before
                .get(&address)
                .ok_or(SimulationError::StateUnavailable)?;
            accounts.push(AccountChange {
                balance_before: format!(
                    "{:#x}",
                    original.as_ref().map_or(U256::ZERO, |info| info.balance)
                ),
                nonce_before: original.as_ref().map_or(0, |info| info.nonce),
                address: format!("{address:#x}"),
                balance: format!("{:#x}", account.info.balance),
                nonce: account.info.nonce,
            });
        }
        for (slot, value) in account.storage {
            if value.is_changed() {
                storage.push(StorageChange {
                    address: format!("{address:#x}"),
                    slot: format!("{slot:#066x}"),
                    before: format!("{:#066x}", value.original_value()),
                    after: format!("{:#066x}", value.present_value()),
                });
            }
        }
        if accounts.len() > 128 || storage.len() > 128 {
            return Err(SimulationError::Limit);
        }
    }
    accounts.sort_by(|a, b| a.address.cmp(&b.address));
    storage.sort_by(|a, b| (&a.address, &a.slot).cmp(&(&b.address, &b.slot)));
    Ok(SimulationObservation {
        request_commitment: crate::protocol::commitment(request)?,
        block_hash: request.block.hash.clone(),
        engine: engine.into(),
        status,
        gas_used: result.result.tx_gas_used(),
        regular_gas_used: result.result.gas().block_regular_gas_used(),
        state_gas_used: result.result.gas().block_state_gas_used(),
        gas_spent_before_refund: result.result.gas().total_gas_spent(),
        effective_gas_refund: result.result.gas().final_refunded(),
        floor_gas: result.result.gas().floor_gas(),
        output,
        calls,
        logs,
        accounts,
        storage,
        effects_qualified: false,
    })
}
