macro_rules! database {
    ($vm:ident) => {
        use $vm as vm;
        // Map-only state: unknown is an error, including unknown storage on a known
        // account. There is deliberately no pipe, endpoint, source or network callback.
        use crate::{SimulationError, StateQuery, StateValue, admission};
        use std::collections::{BTreeMap, BTreeSet};
        use vm::{
            Database,
            primitives::{Address, B256, Bytes, U256},
            state::{AccountInfo, Bytecode},
        };

        #[derive(Clone)]
        pub(super) struct FrozenDb {
            pub(super) accounts: BTreeMap<Address, Option<AccountInfo>>,
            codes: BTreeMap<B256, Bytecode>,
            slots: BTreeMap<(Address, U256), U256>,
            blocks: BTreeMap<u64, B256>,
            missing: Option<crate::replay::MissingQuery>,
        }
        impl std::fmt::Debug for FrozenDb {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("FrozenDb").finish_non_exhaustive()
            }
        }
        impl FrozenDb {
            pub(super) fn new_recording(
                request: &crate::SimulationRequest,
                facts: &[(StateQuery, StateValue)],
                missing: Option<crate::replay::MissingQuery>,
            ) -> Result<Self, SimulationError> {
                // Bound/validate fields before serialising the aggregate. The
                // public host API must not allocate an unbounded JSON buffer.
                if facts.len() > crate::MAX_STATE_QUERIES as usize {
                    return Err(SimulationError::Limit);
                }
                for (query, value) in facts {
                    crate::replay::validate_fact(request, query, value)?;
                }
                if facts.is_empty()
                    || facts.len() > crate::MAX_STATE_QUERIES as usize
                    || serde_json::to_vec(facts)
                        .map_err(|_| SimulationError::Protocol)?
                        .len()
                        > crate::MAX_STATE_BYTES
                {
                    return Err(SimulationError::Limit);
                }
                let mut result = Self {
                    accounts: BTreeMap::new(),
                    codes: BTreeMap::new(),
                    slots: BTreeMap::new(),
                    blocks: BTreeMap::new(),
                    missing,
                };
                let mut keys = BTreeSet::new();
                for (query, value) in facts {
                    admission::validate_query(query, request.block.number)?;
                    if !keys.insert(
                        serde_json::to_string(query)
                            .map_err(|_| SimulationError::Protocol)?
                            .to_ascii_lowercase(),
                    ) {
                        return Err(SimulationError::InvalidInput);
                    }
                    match (query, value) {
                        (
                            StateQuery::Account { address },
                            StateValue::Account {
                                exists,
                                balance,
                                nonce,
                                code,
                            },
                        ) => {
                            let address = admission::address(address)?;
                            let balance = admission::quantity(balance)?;
                            let bytes = admission::data(code, 24_576)?;
                            if bytes.starts_with(&[0xef, 0x01, 0x00]) {
                                return Err(SimulationError::UnsupportedExecution);
                            }
                            if !exists
                                && (balance != U256::ZERO || *nonce != 0 || !bytes.is_empty())
                            {
                                return Err(SimulationError::InconsistentState);
                            }
                            let account = if *exists {
                                let code = Bytecode::new_raw_checked(Bytes::from(bytes))
                                    .map_err(|_| SimulationError::UnsupportedExecution)?;
                                let hash = code.hash_slow();
                                result.codes.insert(hash, code.clone());
                                Some(AccountInfo {
                                    balance,
                                    nonce: *nonce,
                                    code_hash: hash,
                                    code: Some(code),
                                    ..AccountInfo::default()
                                })
                            } else {
                                None
                            };
                            result.accounts.insert(address, account);
                        }
                        (StateQuery::Storage { address, slot }, StateValue::Storage { value }) => {
                            result.slots.insert(
                                (
                                    admission::address(address)?,
                                    U256::from_be_bytes(admission::hash(slot)?.0),
                                ),
                                U256::from_be_bytes(admission::hash(value)?.0),
                            );
                        }
                        (StateQuery::BlockHash { number }, StateValue::BlockHash { hash }) => {
                            result.blocks.insert(*number, admission::hash(hash)?);
                        }
                        _ => return Err(SimulationError::Protocol),
                    }
                }
                if result
                    .slots
                    .keys()
                    .any(|(address, _)| !result.accounts.contains_key(address))
                {
                    return Err(SimulationError::StateUnavailable);
                }
                if result.slots.iter().any(|((address, _), value)| {
                    result.accounts.get(address) == Some(&None) && !value.is_zero()
                }) {
                    return Err(SimulationError::InconsistentState);
                }
                Ok(result)
            }
        }
        impl FrozenDb {
            fn unavailable(&self, query: StateQuery) -> SimulationError {
                if let Some(missing) = &self.missing {
                    let Ok(mut slot) = missing.lock() else {
                        return SimulationError::Protocol;
                    };
                    if slot.is_none() {
                        *slot = Some(query);
                    }
                }
                SimulationError::StateUnavailable
            }
        }
        impl Database for FrozenDb {
            type Error = SimulationError;
            fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
                self.accounts.get(&address).cloned().ok_or_else(|| {
                    self.unavailable(StateQuery::Account {
                        address: format!("{address:#x}"),
                    })
                })
            }
            fn code_by_hash(&mut self, hash: B256) -> Result<Bytecode, Self::Error> {
                self.codes
                    .get(&hash)
                    .cloned()
                    .ok_or(SimulationError::StateUnavailable)
            }
            fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
                self.slots.get(&(address, slot)).copied().ok_or_else(|| {
                    self.unavailable(StateQuery::Storage {
                        address: format!("{address:#x}"),
                        slot: format!("{slot:#066x}"),
                    })
                })
            }
            fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
                self.blocks
                    .get(&number)
                    .copied()
                    .ok_or_else(|| self.unavailable(StateQuery::BlockHash { number }))
            }
        }
    };
}
pub(super) use database;
