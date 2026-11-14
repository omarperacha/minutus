//! Opaque evidence minted only by the proof-checked execution owner.
use alloy_primitives::{Address, Bytes, U256};
use alloy_trie::TrieAccount;
use evm_simulation::{SimulationObservation, SimulationRequest};
use std::collections::BTreeMap;

/// Complete proved account data, never supplied by a model or worker projection.
#[derive(Clone, Eq, PartialEq)]
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
#[derive(Clone)]
pub(crate) struct ProvedState {
    accounts: BTreeMap<Address, ProvedAccount>,
    slots: BTreeMap<(Address, U256), U256>,
}
impl ProvedState {
    /// Combines already proved records only after the owning caller has bound
    /// both to one exact header. This is not a network-source seed constructor.
    pub(crate) fn merge(&mut self, extra: Self) -> Result<(), evm_simulation::SimulationError> {
        let mut merged = self.clone();
        for (address, account) in extra.accounts {
            if merged
                .accounts
                .get(&address)
                .is_some_and(|known| known != &account)
            {
                return Err(evm_simulation::SimulationError::InconsistentState);
            }
            merged.accounts.insert(address, account);
        }
        for (key, value) in extra.slots {
            if merged.slots.get(&key).is_some_and(|known| *known != value) {
                return Err(evm_simulation::SimulationError::InconsistentState);
            }
            merged.slots.insert(key, value);
        }
        if merged.accounts.len() > 128 || merged.slots.len() > 128 {
            return Err(evm_simulation::SimulationError::Limit);
        }
        *self = merged;
        Ok(())
    }
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
    pub(crate) fn account_addresses(&self) -> impl Iterator<Item = Address> + '_ {
        self.accounts.keys().copied()
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

#[cfg(test)]
mod tests {
    use super::*;
    use evm_simulation::SimulationError;
    fn state(value: u64, balance: u64) -> ProvedState {
        ProvedState::new(
            BTreeMap::from([(
                Address::ZERO,
                (
                    true,
                    TrieAccount {
                        balance: U256::from(balance),
                        ..TrieAccount::default()
                    },
                    Bytes::new(),
                ),
            )]),
            BTreeMap::from([((Address::ZERO, U256::ZERO), U256::from(value))]),
        )
    }
    #[test]
    fn proof_record_merge_rejects_account_or_slot_conflicts_without_partial_mutation() {
        for extra in [state(2, 1), state(1, 2)] {
            let mut original = state(1, 1);
            assert_eq!(
                original.merge(extra),
                Err(SimulationError::InconsistentState)
            );
            assert_eq!(
                original.account(Address::ZERO).map(ProvedAccount::balance),
                Some(U256::from(1))
            );
            assert_eq!(
                original.slot(Address::ZERO, U256::ZERO),
                Some(U256::from(1))
            );
        }
    }
    #[test]
    fn proof_record_merge_accepts_exact_overlap_and_enforces_the_retained_bound()
    -> Result<(), SimulationError> {
        let mut original = state(1, 1);
        original.merge(state(1, 1))?;
        let slots = (1..=128)
            .map(|index| ((Address::ZERO, U256::from(index)), U256::ZERO))
            .collect();
        assert_eq!(
            original.merge(ProvedState::new(BTreeMap::new(), slots)),
            Err(SimulationError::Limit)
        );
        assert!(original.slot(Address::ZERO, U256::from(1)).is_none());
        Ok(())
    }
}
