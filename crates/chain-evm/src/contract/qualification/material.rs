//! Reviewed direct-code token semantics, independent of native fee settlement.
//! No worker observation can construct this certificate or choose its outputs.
use super::{
    ContractAction, QualificationError,
    action::{allowance_slot, balance_slot, parse_address},
    profile,
};
use crate::local_simulation::ProvedState;
use alloy_primitives::{Address, U256, keccak256};
use evm_simulation::{
    LogObservation, SimulationObservation, StorageChange, offline::OfflineMaterialExpectation,
};
use serde::Serialize;
use std::collections::BTreeMap;

/// Exact independently checked ERC-20 balance state; labels are absent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenBalanceEffect {
    /// Exact token-holder account.
    pub account: String,
    /// Proved raw balance before execution.
    pub before: String,
    /// Independently expected raw balance; execution is checked separately.
    pub after: String,
}
/// Exact authority change; maximum uint256 is shown as explicit unlimited authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceEffect {
    /// Exact owner account.
    pub owner: String,
    /// Exact spender account.
    pub spender: String,
    /// Proved raw allowance before execution.
    pub before: String,
    /// Expected and simulated raw allowance afterwards.
    pub after: String,
    /// Whether the final amount is maximum uint256.
    pub unlimited: bool,
}

/// Exact material token changes from proved state and reviewed source semantics.
/// It makes no claim about execution success, gas or native fees before signing.
/// Creation provenance and snapshot identity remain separate required certificates.
pub struct QualifiedMaterialEffects {
    action: ContractAction,
    balances: Vec<TokenBalanceEffect>,
    allowance: Option<AllowanceEffect>,
    storage: Vec<StorageChange>,
    log: LogObservation,
    offline: OfflineMaterialExpectation,
}
impl QualifiedMaterialEffects {
    pub(super) fn from_state(
        state: &ProvedState,
        target: Address,
        action: ContractAction,
    ) -> Result<Self, QualificationError> {
        let account = state
            .account(target)
            .ok_or(QualificationError::MissingState)?;
        if !account.exists() {
            return Err(QualificationError::MissingState);
        }
        profile::verify(account.code())?;
        // This exact source/runtime's state-changing paths depend only on calldata,
        // CALLER and proved token slots. CALLVALUE enforces the zero-value gate.
        // Reviewed instructions contain no balance, fee, gas, block-context,
        // external-call, precompile, creation or destruction operation. Signed
        // Base fees may change native settlement, never these material outcomes.
        // An out-of-gas or failed signed execution must still reject offline.
        let mut expected = ExpectedState {
            state,
            target,
            slots: BTreeMap::new(),
            balances: BTreeMap::new(),
            allowance: None,
        };
        expected.apply(&action)?;
        let storage = expected
            .slots
            .iter()
            .filter(|(_, pair)| pair.0 != pair.1)
            .map(|(slot, pair)| StorageChange {
                address: format!("{target:#x}"),
                slot: format!("{slot:#066x}"),
                before: format!("{:#066x}", pair.0),
                after: format!("{:#066x}", pair.1),
            })
            .collect::<Vec<_>>();
        let log = expected_event(target, &action)?;
        let offline = OfflineMaterialExpectation::new(
            storage.clone(),
            vec![log.clone()],
            format!("0x{:064x}", 1),
        )?;
        Ok(Self {
            action,
            balances: expected.balances.into_values().collect(),
            allowance: expected.allowance,
            storage,
            log,
            offline,
        })
    }
    /// Exact source-interpreted action; arbitrary labels are absent.
    #[must_use]
    pub const fn action(&self) -> &ContractAction {
        &self.action
    }
    /// Proved before values and independent expected raw balance outcomes.
    #[must_use]
    pub fn balances(&self) -> &[TokenBalanceEffect] {
        &self.balances
    }
    /// Exact finite or explicitly unlimited authority change.
    #[must_use]
    pub const fn allowance(&self) -> Option<&AllowanceEffect> {
        self.allowance.as_ref()
    }
    pub(crate) const fn offline_expectation(&self) -> &OfflineMaterialExpectation {
        &self.offline
    }
    pub(super) fn verify_observation(
        &self,
        observed: &SimulationObservation,
    ) -> Result<(), QualificationError> {
        let mut changed = self
            .storage
            .iter()
            .map(|change| (&change.slot, change))
            .collect::<BTreeMap<_, _>>();
        for actual in &observed.storage {
            let expected = changed
                .remove(&actual.slot)
                .ok_or(QualificationError::UncoveredEffects)?;
            if actual.address != expected.address
                || actual.before != expected.before
                || actual.after != expected.after
            {
                return Err(QualificationError::UncoveredEffects);
            }
        }
        if !changed.is_empty() {
            return Err(QualificationError::UncoveredEffects);
        }
        let [log] = observed.logs.as_slice() else {
            return Err(QualificationError::UncoveredEffects);
        };
        if log.address != self.log.address
            || log.topics != self.log.topics
            || log.data != self.log.data
        {
            return Err(QualificationError::UncoveredEffects);
        }
        Ok(())
    }
}

struct ExpectedState<'a> {
    state: &'a ProvedState,
    target: Address,
    slots: BTreeMap<U256, (U256, U256)>,
    balances: BTreeMap<Address, TokenBalanceEffect>,
    allowance: Option<AllowanceEffect>,
}
impl ExpectedState<'_> {
    fn word(&mut self, key: U256) -> Result<&mut (U256, U256), QualificationError> {
        if !self.slots.contains_key(&key) {
            let value = self
                .state
                .slot(self.target, key)
                .ok_or(QualificationError::MissingState)?;
            self.slots.insert(key, (value, value));
        }
        self.slots
            .get_mut(&key)
            .ok_or(QualificationError::MissingState)
    }
    fn apply(&mut self, action: &ContractAction) -> Result<(), QualificationError> {
        match action {
            ContractAction::Approve {
                owner,
                spender,
                amount,
            } => {
                let owner = parse_address(owner)?;
                let spender = parse_address(spender)?;
                let amount = decimal(amount)?;
                let word = self.word(allowance_slot(owner, spender))?;
                if word.0 != U256::ZERO && amount != U256::ZERO {
                    return Err(QualificationError::ApprovalRace);
                }
                word.1 = amount;
                self.allowance = Some(AllowanceEffect {
                    owner: format!("{owner:#x}"),
                    spender: format!("{spender:#x}"),
                    before: word.0.to_string(),
                    after: amount.to_string(),
                    unlimited: amount == U256::MAX,
                });
            }
            ContractAction::Transfer { from, to, amount } => {
                self.transfer(parse_address(from)?, parse_address(to)?, decimal(amount)?)?;
            }
            ContractAction::TransferFrom {
                spender,
                from,
                to,
                amount,
            } => {
                let spender = parse_address(spender)?;
                let from = parse_address(from)?;
                let amount = decimal(amount)?;
                let word = self.word(allowance_slot(from, spender))?;
                if word.0 != U256::MAX {
                    word.1 = word
                        .0
                        .checked_sub(amount)
                        .ok_or(QualificationError::Arithmetic)?;
                }
                self.allowance = Some(AllowanceEffect {
                    owner: format!("{from:#x}"),
                    spender: format!("{spender:#x}"),
                    before: word.0.to_string(),
                    after: word.1.to_string(),
                    unlimited: word.1 == U256::MAX,
                });
                self.transfer(from, parse_address(to)?, amount)?;
            }
        }
        Ok(())
    }
    fn transfer(
        &mut self,
        from: Address,
        to: Address,
        amount: U256,
    ) -> Result<(), QualificationError> {
        let source = self.word(balance_slot(from))?;
        source.1 = source
            .1
            .checked_sub(amount)
            .ok_or(QualificationError::Arithmetic)?;
        let destination = self.word(balance_slot(to))?;
        destination.1 = destination
            .1
            .checked_add(amount)
            .ok_or(QualificationError::Arithmetic)?;
        for account in [from, to] {
            let word = *self.word(balance_slot(account))?;
            self.balances.insert(
                account,
                TokenBalanceEffect {
                    account: format!("{account:#x}"),
                    before: word.0.to_string(),
                    after: word.1.to_string(),
                },
            );
        }
        Ok(())
    }
}

fn expected_event(
    target: Address,
    action: &ContractAction,
) -> Result<LogObservation, QualificationError> {
    let (signature, from, to, amount) = match action {
        ContractAction::Approve {
            owner,
            spender,
            amount,
        } => ("Approval(address,address,uint256)", owner, spender, amount),
        ContractAction::Transfer { from, to, amount }
        | ContractAction::TransferFrom {
            from, to, amount, ..
        } => ("Transfer(address,address,uint256)", from, to, amount),
    };
    let from = parse_address(from)?;
    let to = parse_address(to)?;
    Ok(LogObservation {
        address: format!("{target:#x}"),
        topics: vec![
            format!("{:#x}", keccak256(signature)),
            format!("{:#066x}", U256::from_be_slice(from.as_slice())),
            format!("{:#066x}", U256::from_be_slice(to.as_slice())),
        ],
        data: format!("{:#066x}", decimal(amount)?),
    })
}
fn decimal(value: &str) -> Result<U256, QualificationError> {
    U256::from_str_radix(value, 10).map_err(|_| QualificationError::InvalidIntent)
}
