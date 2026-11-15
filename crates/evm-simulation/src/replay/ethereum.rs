use super::TrustedReplayExecution;
mod database {
    super::super::database::database!(revm);
}
mod inspector {
    super::super::inspector::trace!(revm);
}
use crate::{
    MAX_EVM_MEMORY, SimulationControl, SimulationError, SimulationRequest, StateQuery, StateValue,
    admission,
};
use database::FrozenDb;
use inspector::Trace;
use revm as vm;

use vm::{Context, InspectEvm, MainBuilder, MainContext};
mod projection {
    super::super::projection::projection!(revm, revm::context_interface::result::HaltReason);
}
pub(super) fn execute(
    request: &SimulationRequest,
    facts: &[(StateQuery, StateValue)],
    control: &SimulationControl<'_>,
    missing: Option<super::MissingQuery>,
) -> Result<TrustedReplayExecution, SimulationError> {
    let (cfg, engine) = configuration(request)?;
    let mut inspector = Trace::new(control);
    let ctx = Context::mainnet()
        .with_db(FrozenDb::new_recording(request, facts, missing)?)
        .with_block(crate::engine::ethereum::block(request)?)
        .with_cfg(cfg);
    let mut evm = ctx.build_mainnet_with_inspector(&mut inspector);
    let execution = evm.inspect_tx(crate::engine::ethereum::transaction(request)?);
    let before = evm.ctx.journaled_state.database.accounts.clone();
    drop(evm);
    if let Some(fault) = inspector.fault {
        return Err(fault);
    }
    let result = execution.map_err(|error| match error {
        vm::context_interface::result::EVMError::Database(error) => error,
        _ => SimulationError::TransactionRejected,
    })?;
    projection::project(request, control, engine, result, &before, inspector, None)
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
