//! Reuses the same bounded adapter for the two pinned upstream type generations.
//! No interpreter/opcode implementation is supplied here.
macro_rules! adapter {
    ($vm:ident) => {
        use crate::{admission, contract::*, protocol::StatePipe};
        use std::{
            collections::BTreeMap,
            io::{Read, Write},
        };
        use vm::interpreter::interpreter_types::Jumps;
        use vm::{
            Database, Inspector,
            context_interface::ContextTr,
            interpreter::{CallInputs, CallOutcome, Interpreter},
            primitives::{Address, B256, Bytes, U256},
            state::{AccountInfo, Bytecode},
        };
        use $vm as vm;
        impl vm::database_interface::DBErrorMarker for SimulationError {}
        pub(super) struct RemoteDb<R, W> {
            pub(super) pipe: StatePipe<R, W>,
            code: BTreeMap<B256, Bytecode>,
            accounts: BTreeMap<Address, Option<AccountInfo>>,
            slots: BTreeMap<(Address, U256), U256>,
        }
        impl<R, W> std::fmt::Debug for RemoteDb<R, W> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("RemoteDb").finish_non_exhaustive()
            }
        }
        impl<R: Read, W: Write> RemoteDb<R, W> {
            pub(super) fn new(pipe: StatePipe<R, W>) -> Self {
                Self {
                    pipe,
                    code: BTreeMap::new(),
                    accounts: BTreeMap::new(),
                    slots: BTreeMap::new(),
                }
            }
        }
        impl<R: Read, W: Write> Database for RemoteDb<R, W> {
            type Error = SimulationError;
            fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
                if let Some(account) = self.accounts.get(&address) {
                    return Ok(account.clone());
                }
                let StateValue::Account {
                    exists,
                    balance,
                    nonce,
                    code,
                } = self.pipe.query(StateQuery::Account {
                    address: format!("{address:#x}"),
                })?
                else {
                    return Err(SimulationError::Protocol);
                };
                let balance = admission::quantity(&balance)?;
                let bytes = admission::data(&code, 24_576)?;
                if bytes.starts_with(&[0xef, 0x01, 0x00]) {
                    return Err(SimulationError::UnsupportedExecution);
                }
                if !exists && (balance != U256::ZERO || nonce != 0 || !bytes.is_empty()) {
                    return Err(SimulationError::InconsistentState);
                }
                let account = if exists {
                    let code = Bytecode::new_raw_checked(Bytes::from(bytes))
                        .map_err(|_| SimulationError::UnsupportedExecution)?;
                    let hash = code.hash_slow();
                    self.code.insert(hash, code.clone());
                    Some(AccountInfo {
                        balance,
                        nonce,
                        code_hash: hash,
                        code: Some(code),
                        ..AccountInfo::default()
                    })
                } else {
                    None
                };
                self.accounts.insert(address, account.clone());
                Ok(account)
            }
            fn code_by_hash(&mut self, hash: B256) -> Result<Bytecode, Self::Error> {
                self.code
                    .get(&hash)
                    .cloned()
                    .ok_or(SimulationError::StateUnavailable)
            }
            fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
                if let Some(value) = self.slots.get(&(address, index)) {
                    return Ok(*value);
                }
                let StateValue::Storage { value } = self.pipe.query(StateQuery::Storage {
                    address: format!("{address:#x}"),
                    slot: format!("{index:#066x}"),
                })?
                else {
                    return Err(SimulationError::Protocol);
                };
                let value = U256::from_be_bytes(admission::hash(&value)?.0);
                self.slots.insert((address, index), value);
                Ok(value)
            }
            fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
                let StateValue::BlockHash { hash } =
                    self.pipe.query(StateQuery::BlockHash { number })?
                else {
                    return Err(SimulationError::Protocol);
                };
                admission::hash(&hash)
            }
        }
        #[derive(Default)]
        pub(crate) struct BoundedInspector {
            pub(crate) fault: Option<SimulationError>,
            pub(crate) calls: Vec<CallObservation>,
            steps: u64,
            depth: usize,
        }
        impl<CTX: ContextTr> Inspector<CTX> for BoundedInspector {
            fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
                self.steps += 1;
                if self.steps > MAX_STEPS {
                    self.fault = Some(SimulationError::Limit);
                }
                // Creation/destruction and their version-specific state effects
                // are intentionally unsupported, including Revm42 edge cases.
                if matches!(interp.bytecode.opcode(), 0xf0 | 0xf5 | 0xff) {
                    self.fault = Some(SimulationError::UnsupportedExecution);
                }
                if self.fault.is_some() {
                    interp.halt_oog();
                }
            }
            fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
                if self.fault.is_some() {
                    interp.halt_oog();
                }
            }
            fn call(&mut self, _context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
                self.depth += 1;
                if self.depth > MAX_DEPTH || self.calls.len() >= 128 {
                    self.fault = Some(SimulationError::Limit);
                }
                if self.fault.is_some() {
                    return Some(CallOutcome::new_oog(
                        inputs.gas_limit,
                        inputs.return_memory_offset.clone(),
                        0,
                    ));
                }
                if self.fault.is_none() {
                    self.calls.push(CallObservation {
                        target: format!("{:#x}", inputs.target_address),
                        code_address: format!("{:#x}", inputs.bytecode_address),
                        caller: format!("{:#x}", inputs.caller),
                        value: format!("{:#x}", inputs.call_value()),
                        depth: self.depth,
                    });
                }
                None
            }
            fn call_end(
                &mut self,
                _context: &mut CTX,
                _inputs: &CallInputs,
                _outcome: &mut CallOutcome,
            ) {
                self.depth = self.depth.saturating_sub(1);
            }
        }
        pub(crate) fn block(
            request: &SimulationRequest,
        ) -> Result<vm::context::BlockEnv, SimulationError> {
            let b = &request.block;
            let mut block = vm::context::BlockEnv {
                number: U256::from(b.number),
                // Admission requires the exact slot for Amsterdam and rejects
                // unexpected post-fork fields for the older reviewed windows.
                slot_num: b.slot_number.unwrap_or(0),
                timestamp: U256::from(b.timestamp),
                gas_limit: b.gas_limit,
                basefee: b.base_fee,
                beneficiary: admission::address(&b.beneficiary)?,
                prevrandao: Some(admission::hash(&b.prevrandao)?),
                ..vm::context::BlockEnv::default()
            };
            block.set_blob_excess_gas_and_price(
                b.excess_blob_gas,
                admission::blob_fraction(request.network, b.timestamp),
            );
            Ok(block)
        }
        pub(crate) fn transaction(
            request: &SimulationRequest,
        ) -> Result<vm::context::TxEnv, SimulationError> {
            let tx = &request.transaction;
            Ok(vm::context::TxEnv {
                tx_type: 2,
                caller: admission::address(&tx.caller)?,
                kind: vm::primitives::TxKind::Call(admission::address(&tx.target)?),
                value: admission::quantity(&tx.value)?,
                data: Bytes::from(admission::data(&tx.calldata, 8192)?),
                gas_limit: tx.gas_limit,
                gas_price: tx.max_fee_per_gas,
                gas_priority_fee: Some(tx.priority_fee_per_gas),
                nonce: tx.nonce,
                chain_id: Some(request.network.chain_id()),
                ..vm::context::TxEnv::default()
            })
        }
    };
}
pub(super) use adapter;
