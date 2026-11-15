super::adapter::adapter!(revm);
use vm::{Context, InspectEvm, MainBuilder, MainContext};
pub(super) fn execute<R: Read, W: Write>(
    request: &SimulationRequest,
    pipe: StatePipe<R, W>,
) -> Result<SimulationObservation, SimulationError> {
    let (cfg, engine) = configuration(request)?;
    let mut inspector = BoundedInspector::default();
    let ctx = Context::mainnet()
        .with_db(RemoteDb::new(pipe))
        .with_block(block(request)?)
        .with_cfg(cfg);
    let mut evm = ctx.build_mainnet_with_inspector(&mut inspector);
    let execution = evm.inspect_tx(transaction(request)?);
    let before = evm.ctx.journaled_state.database.accounts.clone();
    drop(evm);
    if let Some(fault) = inspector.fault {
        return Err(fault);
    }
    let result = execution.map_err(|error| match error {
        vm::context_interface::result::EVMError::Database(error) => error,
        _ => SimulationError::TransactionRejected,
    })?;
    let (status, output) = execution_outcome(&result.result)?;
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
        calls: inspector.calls,
        logs,
        accounts,
        storage,
        effects_qualified: false,
    })
}

fn execution_outcome(
    result: &vm::context_interface::result::ExecutionResult,
) -> Result<(ExecutionStatus, String), SimulationError> {
    let status = if result.is_success() {
        ExecutionStatus::Succeeded
    } else if matches!(
        result,
        vm::context_interface::result::ExecutionResult::Revert { .. }
    ) {
        ExecutionStatus::Reverted
    } else {
        ExecutionStatus::Halted
    };
    let output = result
        .output()
        .map_or_else(|| "0x".into(), |bytes| format!("{bytes:#x}"));
    if output.len() > 8192 * 2 + 2 {
        return Err(SimulationError::Limit);
    }
    Ok((status, output))
}

fn configuration(
    request: &SimulationRequest,
) -> Result<(vm::context::CfgEnv, &'static str), SimulationError> {
    let (spec, engine) = match admission::validate(request)? {
        admission::Rules::Osaka => (vm::primitives::hardfork::SpecId::OSAKA, "revm43.0.3/Osaka"),
        admission::Rules::Amsterdam => (
            vm::primitives::hardfork::SpecId::AMSTERDAM,
            "revm43.0.3/Amsterdam",
        ),
        admission::Rules::BaseCobalt | admission::Rules::BaseDenim => {
            return Err(SimulationError::UnsupportedRules);
        }
    };
    let mut cfg = vm::context::CfgEnv::new_with_spec(spec).with_mainnet_gas_params();
    cfg.chain_id = request.network.chain_id();
    cfg.memory_limit = MAX_EVM_MEMORY;
    Ok((cfg, engine))
}
