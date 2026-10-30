//! Opaque evidence minted only by the proof-checked execution owner.
use alloy_primitives::{Address, Bytes, U256};
use alloy_trie::TrieAccount;
use evm_simulation::{SimulationObservation, SimulationRequest};
use std::collections::BTreeMap;

/// Complete proved account data, never supplied by a model or worker projection.
pub(crate) struct ProvedAccount {
    exists: bool,
    account: TrieAccount,
    code: Bytes,
}
impl ProvedAccount {
    pub(crate) const fn exists(&self) -> bool {
        self.exists
    }
    pub(crate) const fn balance(&self) -> U256 {
        self.account.balance
    }
    pub(crate) const fn nonce(&self) -> u64 {
        self.account.nonce
    }
    pub(crate) fn code(&self) -> &[u8] {
        &self.code
    }
}
/// Complete bounded account/slot evidence from one header state root.
pub(crate) struct ProvedState {
    accounts: BTreeMap<Address, ProvedAccount>,
    slots: BTreeMap<(Address, U256), U256>,
}
impl ProvedState {
    #[cfg(test)]
    pub(crate) fn test_fixture(
        accounts: BTreeMap<Address, (bool, TrieAccount, Bytes)>,
        slots: BTreeMap<(Address, U256), U256>,
    ) -> Self {
        Self::new(accounts, slots)
    }
    pub(super) fn new(
        accounts: BTreeMap<Address, (bool, TrieAccount, Bytes)>,
        slots: BTreeMap<(Address, U256), U256>,
    ) -> Self {
        Self {
            accounts: accounts
                .into_iter()
                .map(|(address, (exists, account, code))| {
                    (
                        address,
                        ProvedAccount {
                            exists,
                            account,
                            code,
                        },
                    )
                })
                .collect(),
            slots,
        }
    }
    pub(crate) fn account(&self, address: Address) -> Option<&ProvedAccount> {
        self.accounts.get(&address)
    }
    pub(crate) fn slot(&self, address: Address, slot: U256) -> Option<U256> {
        self.slots.get(&(address, slot)).copied()
    }
}
/// A successful isolated run with exact proof-checked pre-state and canonical
/// checks, not deserialisable or constructible from a raw observation/digest.
pub(crate) struct ProvedSimulation {
    request: SimulationRequest,
    observation: SimulationObservation,
    state: ProvedState,
}
impl ProvedSimulation {
    /// Synthetic sealed evidence is available only to this crate's unit tests.
    #[cfg(test)]
    pub(crate) fn test_fixture(
        request: SimulationRequest,
        observation: SimulationObservation,
        accounts: BTreeMap<Address, (bool, TrieAccount, Bytes)>,
        slots: BTreeMap<(Address, U256), U256>,
    ) -> Self {
        Self::new(request, observation, ProvedState::new(accounts, slots))
    }

    pub(super) fn new(
        request: SimulationRequest,
        observation: SimulationObservation,
        state: ProvedState,
    ) -> Self {
        Self {
            request,
            observation,
            state,
        }
    }
    pub(crate) const fn request(&self) -> &SimulationRequest {
        &self.request
    }
    pub(crate) const fn observation(&self) -> &SimulationObservation {
        &self.observation
    }
    pub(crate) const fn state(&self) -> &ProvedState {
        &self.state
    }
    pub(super) fn into_observation(self) -> SimulationObservation {
        self.observation
    }
}
