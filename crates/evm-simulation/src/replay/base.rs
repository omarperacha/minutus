//! Explicit application-frame preview, never a complete unsigned Base transaction.
//! Maintained native intrinsic/warming/frame/refund routines supply execution;
//! a proved all-signature prepayment and conservative noninterference guard make
//! its application effects comparable to genuine signed offline execution.
mod database {
    super::super::database::database!(revm_base);
}
mod inspector {
    super::super::inspector::trace!(revm_base);
}
mod projection {
    super::super::projection::projection!(revm_base, base_evm::BaseHaltReason);
}
use super::TrustedReplayExecution;
use crate::{
    SimulationControl, SimulationError, SimulationRequest, StateQuery, StateValue, admission,
    offline::FrozenBaseRequest,
};
use base_evm::{
    BaseHandler, BaseSpecId, BaseTransaction, BaseTransactionError, BaseUpgrade, Builder,
    DefaultBase,
};
use revm_base::{
    Context,
    context::journaled_state::account::JournaledAccountTr,
    context_interface::{ContextTr, JournalTr},
    handler::{
        EthFrame, Handler, post_execution,
        pre_execution::validate_account_nonce_and_code_with_components,
    },
    inspector::InspectorHandler,
    interpreter::interpreter::EthInterpreter,
    primitives::{Address, address},
};

type Error = revm_base::context_interface::result::EVMError<SimulationError, BaseTransactionError>;
pub(super) fn execute(
    request: &SimulationRequest,
    facts: &[(StateQuery, StateValue)],
    control: &SimulationControl<'_>,
    missing: Option<super::MissingQuery>,
) -> Result<TrustedReplayExecution, SimulationError> {
    let snapshot = FrozenBaseRequest::from_replay_facts(request, facts)?;
    let fee_ceiling = snapshot.fee_ceiling()?.total_wei();
    let ceiling = admission::quantity(&fee_ceiling)?;
    let spec = BaseSpecId::new(match admission::validate(request)? {
        admission::Rules::BaseCobalt => BaseUpgrade::Cobalt,
        admission::Rules::BaseDenim => BaseUpgrade::Denim,
        _ => return Err(SimulationError::UnsupportedRules),
    });
    let mut cfg = revm_base::context::CfgEnv::new_with_spec(spec);
    cfg.chain_id = request.network.chain_id();
    cfg.memory_limit = crate::MAX_EVM_MEMORY;
    let mut inspector = inspector::Trace::new(control);
    inspector.base_application = true;
    let db = database::FrozenDb::new_recording(request, facts, missing)?;
    let before = db.accounts.clone();
    let mut ctx = Context::base()
        .with_db(db)
        .with_block(crate::engine::base::block(request)?)
        .with_cfg(cfg);
    ctx.tx = BaseTransaction::new(crate::engine::base::transaction(request)?);
    let admin = activation_admin(request.network)?;
    // Maintained static and dynamic lookup only: never execute a precompile in
    // the trusted host before discovering that the call is outside this scope.
    let precompiles = base_evm::BasePrecompiles::new_with_spec(spec)
        .with_activation_admin_address(Some(admin))
        .install();
    inspector.precompile_guard = Some(Box::new(move |address| precompiles.get(&address).is_some()));
    let mut evm =
        ctx.build_with_inspector_and_activation_admin_address(&mut inspector, Some(admin));
    let mut handler = BaseHandler::<_, Error, EthFrame<EthInterpreter>>::new();
    // Deliberately use maintained ordinary transaction validation, not Base's
    // full-envelope admission: this type certifies only application frames.
    handler
        .mainnet
        .validate_env(&mut evm)
        .map_err(|error| failure(&error))?;
    let intrinsic = handler
        .validate_initial_tx_gas(&mut evm)
        .map_err(|error| failure(&error))?;
    {
        let (block, tx, cfg, journal, _, _) = evm.ctx_mut().all_mut();
        let _ = block;
        let mut caller = journal
            .load_account_with_code_mut(admission::address(&request.transaction.caller)?)?
            .data;
        validate_account_nonce_and_code_with_components(&caller.account().info, tx, cfg)
            .map_err(|_| SimulationError::TransactionRejected)?;
        let balance = caller
            .account()
            .info
            .balance
            .checked_sub(ceiling)
            .ok_or(SimulationError::TransactionRejected)?;
        caller.set_balance(balance);
        caller.bump_nonce();
    }
    let mut gas = handler.tx_gas(&mut evm, &intrinsic);
    let pre = handler
        .pre_execution(&mut evm, &mut gas)
        .map_err(|error| failure(&error))?
        .ok_or(SimulationError::TransactionRejected)?;
    if pre.eip7702_refund != 0 {
        return Err(SimulationError::UnsupportedExecution);
    }
    let mut frame = handler
        .inspect_execution(&mut evm, pre.checkpoint, &mut gas)
        .map_err(|error| failure(&error))?
        .ok_or(SimulationError::TransactionRejected)?;
    handler
        .refund(&mut evm, &mut frame, 0)
        .map_err(|error| failure(&error))?;
    let result_gas = post_execution::build_result_gas(
        frame.instruction_result().is_halt(),
        frame.gas(),
        intrinsic,
    );
    handler.eip7623_check_gas_floor(&mut evm, &mut frame, intrinsic);
    let result = handler
        .execution_result(&mut evm, frame, result_gas)
        .map_err(|error| failure(&error))?;
    let state = evm.ctx_mut().journal_mut().finalize();
    drop(evm);
    if let Some(error) = inspector.fault {
        return Err(error);
    }
    if !result.is_success() {
        return Err(SimulationError::TransactionRejected);
    }
    let result = revm_base::context_interface::result::ResultAndState::new(result, state);
    projection::project(
        request,
        control,
        "base1.5.3/application-frame",
        result,
        &before,
        inspector,
        Some(fee_ceiling),
    )
}
fn activation_admin(network: crate::SimulationNetwork) -> Result<Address, SimulationError> {
    match network {
        crate::SimulationNetwork::BaseMainnet => {
            Ok(address!("ce3a3bee7e72e2a24079f3c0cb3b97740ed425a9"))
        }
        crate::SimulationNetwork::BaseSepolia => {
            Ok(address!("5f43072722f59964d886cbb507f6a85ca0759d42"))
        }
        _ => Err(SimulationError::UnsupportedRules),
    }
}
fn failure(error: &Error) -> SimulationError {
    match error {
        Error::Database(error) => *error,
        _ => SimulationError::TransactionRejected,
    }
}
