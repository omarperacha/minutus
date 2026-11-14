//! Map-only state: unknown is an error, including unknown storage on a known
//! account. There is deliberately no pipe, endpoint, source or network callback.
use super::snapshot::{FrozenBaseRequest, key};
use crate::{SimulationError, StateQuery, StateValue, admission};
use revm_base::{
    Database,
    primitives::{Address, B256, Bytes, U256},
    state::{AccountInfo, Bytecode},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub(super) struct FrozenDb {
    pub(super) accounts: BTreeMap<Address, Option<AccountInfo>>,
    codes: BTreeMap<B256, Bytecode>,
    slots: BTreeMap<(Address, U256), U256>,
    blocks: BTreeMap<u64, B256>,
}
impl std::fmt::Debug for FrozenDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrozenDb").finish_non_exhaustive()
    }
}
impl FrozenDb {
    pub(super) fn new(snapshot: &FrozenBaseRequest) -> Result<Self, SimulationError> {
        let mut result = Self {
            accounts: BTreeMap::new(),
            codes: BTreeMap::new(),
            slots: BTreeMap::new(),
            blocks: BTreeMap::new(),
        };
        let mut keys = BTreeSet::new();
        for fact in &snapshot.facts {
            admission::validate_query(&fact.query, snapshot.request.block.number)?;
            if !keys.insert(key(&fact.query)?) {
                return Err(SimulationError::InvalidInput);
            }
            match (&fact.query, &fact.value) {
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
                    if !exists && (balance != U256::ZERO || *nonce != 0 || !bytes.is_empty()) {
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
impl Database for FrozenDb {
    type Error = SimulationError;
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.accounts
            .get(&address)
            .cloned()
            .ok_or(SimulationError::StateUnavailable)
    }
    fn code_by_hash(&mut self, hash: B256) -> Result<Bytecode, Self::Error> {
        self.codes
            .get(&hash)
            .cloned()
            .ok_or(SimulationError::StateUnavailable)
    }
    fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        self.slots
            .get(&(address, slot))
            .copied()
            .ok_or(SimulationError::StateUnavailable)
    }
    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.blocks
            .get(&number)
            .copied()
            .ok_or(SimulationError::StateUnavailable)
    }
}
