//! Full upstream Base handler; unsigned envelopes cannot bypass its fee policy.
super::adapter::adapter!(revm_base);
use base_evm::{BaseSpecId, BaseTransaction, BaseUpgrade, Builder, DefaultBase};
use vm::{Context, InspectEvm};

pub(super) fn execute<R: Read, W: Write>(
    request: &SimulationRequest,
    pipe: StatePipe<R, W>,
) -> Result<SimulationObservation, SimulationError> {
    let upgrade = match crate::admission::validate(request)? {
        crate::admission::Rules::BaseCobalt => BaseUpgrade::Cobalt,
        crate::admission::Rules::BaseDenim => BaseUpgrade::Denim,
        crate::admission::Rules::Osaka | crate::admission::Rules::Amsterdam => {
            return Err(SimulationError::UnsupportedRules);
        }
    };
    let mut cfg = vm::context::CfgEnv::new_with_spec(BaseSpecId::new(upgrade));
    cfg.chain_id = request.network.chain_id();
    cfg.memory_limit = MAX_EVM_MEMORY;
    let mut inspector = BoundedInspector::default();
    let ctx = Context::base()
        .with_db(RemoteDb::new(pipe))
        .with_block(block(request)?)
        .with_cfg(cfg);
    let mut evm = ctx.build_with_inspector(&mut inspector);
    // Native Base requires the complete ordinary transaction envelope for L1/
    // operator fees. This request type intentionally has no signed/raw envelope
    // input. Preserve upstream rejection, rather than fabricate bytes or switch
    // to a deposit/system transaction to obtain a misleading successful preview.
    let result = evm.inspect_tx(BaseTransaction::new(transaction(request)?));
    match result {
        Err(vm::context_interface::result::EVMError::Transaction(
            base_evm::BaseTransactionError::MissingEnvelopedTx,
        )) => Err(SimulationError::BaseEnvelopeRequired),
        Err(_) => Err(SimulationError::TransactionRejected),
        Ok(_) => Err(SimulationError::UnsupportedExecution),
    }
}
