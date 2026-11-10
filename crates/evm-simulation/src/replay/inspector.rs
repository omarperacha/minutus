macro_rules! trace {
    ($vm:ident) => {
        use crate::replay::{ReplayCall, ReplayCallKind};
        use crate::{MAX_DEPTH, MAX_STEPS, SimulationControl, SimulationError};
        use vm::{
            Inspector,
            context_interface::ContextTr,
            interpreter::{
                CallInputs, CallOutcome, CallScheme, Interpreter, interpreter_types::Jumps,
            },
        };
        use $vm as vm;

        pub(super) struct Trace<'a, 'b> {
            pub(super) control: &'a SimulationControl<'b>,
            pub(super) fault: Option<SimulationError>,
            pub(super) calls: Vec<ReplayCall>,
            pub(super) preimages: Vec<Vec<u8>>,
            pub(super) writes: Vec<(String, String)>,
            pub(super) sensitive: Vec<u8>,
            stack: Vec<usize>,
            steps: u64,
            bytes: usize,
            pub(super) base_application: bool,
            pub(super) precompile_guard: Option<Box<dyn Fn(vm::primitives::Address) -> bool>>,
        }
        impl<'a, 'b> Trace<'a, 'b> {
            pub(super) fn new(control: &'a SimulationControl<'b>) -> Self {
                Self {
                    control,
                    fault: None,
                    calls: Vec::new(),
                    preimages: Vec::new(),
                    writes: Vec::new(),
                    sensitive: Vec::new(),
                    stack: Vec::new(),
                    steps: 0,
                    bytes: 0,
                    base_application: false,
                    precompile_guard: None,
                }
            }
            fn charge(&mut self, bytes: usize) -> Result<(), SimulationError> {
                self.bytes = self
                    .bytes
                    .checked_add(bytes)
                    .ok_or(SimulationError::Limit)?;
                if self.bytes > 256 * 1024 {
                    return Err(SimulationError::Limit);
                }
                Ok(())
            }
            fn observe_step(&mut self, interp: &Interpreter) -> Result<(), SimulationError> {
                self.control.remaining()?;
                self.steps += 1;
                if self.steps > MAX_STEPS {
                    return Err(SimulationError::Limit);
                }
                let opcode = interp.bytecode.opcode();
                if self.base_application && matches!(opcode, 0x31 | 0x47) {
                    return Err(SimulationError::UnsupportedExecution);
                }
                if matches!(opcode, 0xf0 | 0xf5 | 0xff) {
                    return Err(SimulationError::UnsupportedExecution);
                }
                if matches!(opcode, 0x31 | 0x3a | 0x40..=0x4a | 0x5a)
                    && !self.sensitive.contains(&opcode)
                {
                    self.sensitive.push(opcode);
                }
                if opcode == 0x55
                    && let Ok(slot) = interp.stack.peek(0)
                {
                    let key = (
                        format!("{:#x}", interp.input.target_address),
                        format!("{slot:#066x}"),
                    );
                    if !self.writes.contains(&key) {
                        if self.writes.len() >= 128 {
                            return Err(SimulationError::Limit);
                        }
                        self.writes.push(key);
                    }
                }
                if opcode == 0x20
                    && let (Ok(offset), Ok(length)) = (interp.stack.peek(0), interp.stack.peek(1))
                {
                    let length: usize = length.try_into().map_err(|_| SimulationError::Limit)?;
                    if length > 8192 || self.preimages.len() >= 256 {
                        return Err(SimulationError::Limit);
                    }
                    self.charge(length)?;
                    let offset: usize = if length == 0 {
                        0
                    } else {
                        offset.try_into().map_err(|_| SimulationError::Limit)?
                    };
                    let end = offset.checked_add(length).ok_or(SimulationError::Limit)?;
                    if end
                        > usize::try_from(crate::MAX_EVM_MEMORY)
                            .map_err(|_| SimulationError::Limit)?
                    {
                        return Err(SimulationError::Limit);
                    }
                    let mut preimage = vec![0; length];
                    if offset < interp.memory.len() {
                        let available = length.min(interp.memory.len() - offset);
                        preimage[..available]
                            .copy_from_slice(&interp.memory.slice_len(offset, available));
                    }
                    self.preimages.push(preimage);
                }
                Ok(())
            }
        }
        impl<CTX: ContextTr> Inspector<CTX> for Trace<'_, '_> {
            fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
                if self.fault.is_none() {
                    self.fault = self.observe_step(interp).err();
                }
                if self.fault.is_some() {
                    interp.halt_oog();
                }
            }
            fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
                let checked = (|| {
                    self.control.remaining()?;
                    if self
                        .precompile_guard
                        .as_ref()
                        .is_some_and(|blocked| blocked(inputs.bytecode_address))
                    {
                        return Err(SimulationError::UnsupportedExecution);
                    }
                    let address = inputs.bytecode_address.as_slice();
                    if address[..18] == [0; 18] {
                        let low = u16::from_be_bytes([address[18], address[19]]);
                        // Fixed reviewed standard precompile addresses, including
                        // Osaka P256. Return before maintained frame initialisation
                        // can dispatch any native cryptographic implementation.
                        if (1..=0x20).contains(&low) || low == 0x100 {
                            return Err(SimulationError::UnsupportedExecution);
                        }
                    }
                    if self.base_application
                        && (!inputs.call_value().is_zero()
                            || [
                                vm::primitives::address!(
                                    "420000000000000000000000000000000000000f"
                                ),
                                vm::primitives::address!(
                                    "4200000000000000000000000000000000000015"
                                ),
                                vm::primitives::address!(
                                    "4200000000000000000000000000000000000019"
                                ),
                                vm::primitives::address!(
                                    "420000000000000000000000000000000000001a"
                                ),
                                vm::primitives::address!(
                                    "420000000000000000000000000000000000001b"
                                ),
                            ]
                            .contains(&inputs.bytecode_address))
                    {
                        return Err(SimulationError::UnsupportedExecution);
                    }
                    if self.calls.len() >= 128
                        || self.stack.len() >= MAX_DEPTH
                        || inputs.input.len() > 8192
                    {
                        return Err(SimulationError::Limit);
                    }
                    let input = inputs.input.bytes(context);
                    if input.len() != inputs.input.len() {
                        return Err(SimulationError::InconsistentState);
                    }
                    self.charge(input.len())?;
                    let kind = match inputs.scheme {
                        CallScheme::Call => ReplayCallKind::Call,
                        CallScheme::CallCode => ReplayCallKind::CallCode,
                        CallScheme::DelegateCall => ReplayCallKind::DelegateCall,
                        CallScheme::StaticCall => ReplayCallKind::StaticCall,
                    };
                    let index = self.calls.len();
                    self.calls.push(ReplayCall {
                        kind,
                        target: format!("{:#x}", inputs.target_address),
                        code_address: format!("{:#x}", inputs.bytecode_address),
                        caller: format!("{:#x}", inputs.caller),
                        value: format!("{:#x}", inputs.call_value()),
                        input: format!("{input:#x}"),
                        output: "0x".into(),
                        succeeded: false,
                        committed: false,
                        parent: self.stack.last().copied(),
                        depth: self.stack.len() + 1,
                    });
                    self.stack.push(index);
                    Ok(())
                })();
                if self.fault.is_none() {
                    self.fault = checked.err();
                }
                if self.fault.is_some() {
                    Some(CallOutcome::new_oog(
                        inputs.gas_limit,
                        inputs.return_memory_offset.clone(),
                        0,
                    ))
                } else {
                    None
                }
            }
            fn call_end(
                &mut self,
                _context: &mut CTX,
                _inputs: &CallInputs,
                outcome: &mut CallOutcome,
            ) {
                if self.fault.is_some() {
                    return;
                }
                if self.base_application && outcome.was_precompile_called {
                    self.fault = Some(SimulationError::UnsupportedExecution);
                    return;
                }
                let Some(index) = self.stack.pop() else {
                    self.fault = Some(SimulationError::InconsistentState);
                    return;
                };
                if outcome.output().len() > 8192 {
                    self.fault = Some(SimulationError::Limit);
                    return;
                }
                if let Err(error) = self.charge(outcome.output().len()) {
                    self.fault = Some(error);
                    return;
                }
                self.calls[index].output = format!("{:#x}", outcome.output());
                self.calls[index].succeeded = outcome.result.result.is_ok();
            }
        }
    };
}
pub(super) use trace;
