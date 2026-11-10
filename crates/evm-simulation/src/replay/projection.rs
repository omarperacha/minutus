macro_rules! projection {
    ($vm:ident,$halt:ty) => {
        use super::inspector::Trace;
        use crate::replay::{NativeTransfer, ReplayCallKind, TrustedReplayExecution};
        use crate::{
            AccountChange, CallObservation, ExecutionStatus, LogObservation, SimulationControl,
            SimulationError, SimulationObservation, SimulationRequest, StorageChange,
        };
        use vm::primitives::U256;
        use $vm as vm;
        pub(super) fn project(
            request: &SimulationRequest,
            control: &SimulationControl<'_>,
            engine: &str,
            result: vm::context_interface::result::ResultAndState<$halt>,
            before: &std::collections::BTreeMap<
                vm::primitives::Address,
                Option<vm::state::AccountInfo>,
            >,
            mut inspector: Trace<'_, '_>,
            fee_ceiling: Option<String>,
        ) -> Result<TrustedReplayExecution, SimulationError> {
            let (status, output) = execution_outcome(&result.result)?;
            let logs = project_logs(&result.result)?;
            let (accounts, storage) = project_state(result.state, before)?;
            let calls = inspector
                .calls
                .iter()
                .map(|call| CallObservation {
                    target: call.target.clone(),
                    code_address: call.code_address.clone(),
                    caller: call.caller.clone(),
                    value: call.value.clone(),
                    depth: call.depth,
                })
                .collect();
            let observation = SimulationObservation {
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
            };
            control.remaining()?;
            for index in 0..inspector.calls.len() {
                let parent_ok = inspector.calls[index]
                    .parent
                    .is_none_or(|parent| inspector.calls[parent].committed);
                inspector.calls[index].committed = inspector.calls[index].succeeded
                    && parent_ok
                    && status == ExecutionStatus::Succeeded;
            }
            let transfers = inspector
                .calls
                .iter()
                .filter(|call| {
                    call.committed && call.kind == ReplayCallKind::Call && call.value != "0x0"
                })
                .map(|call| NativeTransfer {
                    from: call.caller.clone(),
                    to: call.target.clone(),
                    value: call.value.clone(),
                })
                .collect();
            Ok(TrustedReplayExecution {
                observation,
                calls: inspector.calls,
                preimages: inspector.preimages,
                writes: inspector.writes,
                sensitive: inspector.sensitive,
                transfers,
                base_application: fee_ceiling.is_some(),
                fee_ceiling,
            })
        }

        fn execution_outcome(
            result: &vm::context_interface::result::ExecutionResult<$halt>,
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

        fn project_logs(
            result: &vm::context_interface::result::ExecutionResult<$halt>,
        ) -> Result<Vec<LogObservation>, SimulationError> {
            let mut logs = Vec::new();
            let mut log_bytes = 0;
            if result.is_success() {
                for log in result.logs() {
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
            Ok(logs)
        }
        fn project_state(
            state: vm::state::EvmState,
            before: &std::collections::BTreeMap<
                vm::primitives::Address,
                Option<vm::state::AccountInfo>,
            >,
        ) -> Result<(Vec<AccountChange>, Vec<StorageChange>), SimulationError> {
            let mut accounts = Vec::new();
            let mut storage = Vec::new();
            for (address, account) in state {
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
            Ok((accounts, storage))
        }
    };
}
pub(super) use projection;
