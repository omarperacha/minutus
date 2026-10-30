//! Bounded proof-checked state acquisition; no simulation calldata is sent here.
use super::{header, proof};
use crate::{EvmNetwork, rpc::RpcClient};
use alloy_consensus::Header;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_trie::TrieAccount;
use evm_simulation::{
    SimulationControl, SimulationError, SimulationStateSource, StateQuery, StateValue,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, str::FromStr, time::Duration};

pub(super) trait StateRpc {
    fn value(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, SimulationError>;
}
impl StateRpc for RpcClient<'_> {
    fn value(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, SimulationError> {
        self.value_result_with_timeout(method, params, timeout)
            .map_err(|_| SimulationError::StateUnavailable)
    }
}

pub(super) struct StateSource<R> {
    pub(super) rpc: R,
    pub(super) network: EvmNetwork,
    pub(super) header: Header,
    pub(super) hash: B256,
    pub(super) requests: u32,
    accounts: BTreeMap<Address, (bool, TrieAccount, Bytes)>,
    ancestors: BTreeMap<u64, Header>,
    slots: BTreeMap<(Address, U256), U256>,
}
impl<R: StateRpc> StateSource<R> {
    pub(super) fn new(rpc: R, network: EvmNetwork, header: Header, hash: B256) -> Self {
        Self {
            rpc,
            network,
            header,
            hash,
            requests: 2,
            accounts: BTreeMap::new(),
            ancestors: BTreeMap::new(),
            slots: BTreeMap::new(),
        }
    }
    fn rpc(
        &mut self,
        method: &str,
        params: &Value,
        control: &SimulationControl<'_>,
    ) -> Result<Value, SimulationError> {
        let left = control.remaining()?;
        self.requests += 1;
        if self.requests > 256 {
            return Err(SimulationError::Limit);
        }
        let value = self
            .rpc
            .value(method, params, left.min(Duration::from_secs(1)))?;
        control.remaining()?;
        Ok(value)
    }
    fn proof(
        &mut self,
        address: Address,
        slots: &[String],
        control: &SimulationControl<'_>,
    ) -> Result<Value, SimulationError> {
        self.rpc("eth_getProof",&json!([format!("{address:#x}"),slots,{"blockHash":format!("{:#x}",self.hash),"requireCanonical":true}]),control)
    }
    fn load_account(
        &mut self,
        address: Address,
        control: &SimulationControl<'_>,
    ) -> Result<(bool, TrieAccount, Bytes), SimulationError> {
        if let Some(account) = self.accounts.get(&address) {
            return Ok(account.clone());
        }
        let proof = self.proof(address, &[], control)?;
        let (exists, account) = proof::account(self.header.state_root, address, &proof)?;
        let code=self.rpc("eth_getCode",&json!([format!("{address:#x}"),{"blockHash":format!("{:#x}",self.hash),"requireCanonical":true}]),control)?;
        let code = code
            .as_str()
            .filter(|code| code.len() <= 24_576 * 2 + 2)
            .ok_or(SimulationError::Limit)?;
        let code = Bytes::from_str(code).map_err(|_| SimulationError::InconsistentState)?;
        if keccak256(&code) != account.code_hash {
            return Err(SimulationError::InconsistentState);
        }
        self.accounts
            .insert(address, (exists, account, code.clone()));
        Ok((exists, account, code))
    }
    pub(super) fn into_proved_state(self) -> super::record::ProvedState {
        super::record::ProvedState::new(self.accounts, self.slots)
    }
    pub(super) fn canonical(
        &mut self,
        control: &SimulationControl<'_>,
    ) -> Result<(), SimulationError> {
        let chain = self.rpc("eth_chainId", &json!([]), control)?;
        self.network
            .validate_rpc_chain_id(chain.as_str().ok_or(SimulationError::InconsistentState)?)
            .map_err(|_| SimulationError::InconsistentState)?;
        let block = self.rpc(
            "eth_getBlockByNumber",
            &json!([format!("0x{:x}", self.header.number), false]),
            control,
        )?;
        let (header, _) = header::decode(block, Some(self.hash))?;
        if header.number != self.header.number {
            return Err(SimulationError::InconsistentState);
        }
        Ok(())
    }
}
impl<R: StateRpc> SimulationStateSource for StateSource<R> {
    fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, SimulationError> {
        match query {
            StateQuery::Account { address } => {
                let address =
                    Address::from_str(address).map_err(|_| SimulationError::InvalidInput)?;
                let (exists, account, code) = self.load_account(address, control)?;
                Ok(StateValue::Account {
                    exists,
                    balance: format!("{:#x}", account.balance),
                    nonce: account.nonce,
                    code: format!("{code:#x}"),
                })
            }
            StateQuery::Storage { address, slot } => {
                let address =
                    Address::from_str(address).map_err(|_| SimulationError::InvalidInput)?;
                let slot = U256::from_str(slot).map_err(|_| SimulationError::InvalidInput)?;
                if let Some(value) = self.slots.get(&(address, slot)) {
                    return Ok(StateValue::Storage {
                        value: format!("{value:#066x}"),
                    });
                }
                let (_, account, _) = self.load_account(address, control)?;
                let proof = self.proof(address, &[format!("{slot:#066x}")], control)?;
                let (_, again) = proof::account(self.header.state_root, address, &proof)?;
                if account != again {
                    return Err(SimulationError::InconsistentState);
                }
                let value = proof::storage(account.storage_root, slot, &proof)?;
                self.slots.insert((address, slot), value);
                Ok(StateValue::Storage {
                    value: format!("{value:#066x}"),
                })
            }
            StateQuery::BlockHash { number } => {
                if *number >= self.header.number || self.header.number - number > 256 {
                    return Err(SimulationError::InvalidInput);
                }
                let mut header = self.header.clone();
                while header.number > *number {
                    if let Some(cached) = self.ancestors.get(&(header.number - 1)) {
                        header = cached.clone();
                        continue;
                    }
                    let value = self.rpc(
                        "eth_getBlockByHash",
                        &json!([format!("{:#x}", header.parent_hash), false]),
                        control,
                    )?;
                    let (parent, _) = header::decode(value, Some(header.parent_hash))?;
                    if parent.number.checked_add(1) != Some(header.number) {
                        return Err(SimulationError::InconsistentState);
                    }
                    self.ancestors.insert(parent.number, parent.clone());
                    header = parent;
                }
                Ok(StateValue::BlockHash {
                    hash: format!("{:#x}", header.hash_slow()),
                })
            }
        }
    }
}
