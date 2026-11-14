use super::QualificationError;
use crate::contract::UnsignedContractIntent;
use alloy_primitives::{Address, U256, keccak256};
use evm_simulation::StateQuery;
use serde::Serialize;
use std::{collections::BTreeSet, str::FromStr};

/// Independently interpreted action within a reviewed code family. The value
/// itself is presentation data; only an opaque qualified candidate authorises review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ContractAction {
    /// Set an exact ERC-20 allowance, with zero-first admission.
    Approve {
        /// Exact token owner.
        owner: String,
        /// Exact spender, never a label.
        spender: String,
        /// Exact raw units, including explicit maximum authority.
        amount: String,
    },
    /// Transfer exact ERC-20 raw units.
    Transfer {
        /// Exact debit account.
        from: String,
        /// Exact credit account.
        to: String,
        /// Exact raw units, no trusted token symbol.
        amount: String,
    },
    /// Spend a specific allowance and transfer exact ERC-20 raw units.
    TransferFrom {
        /// Exact allowance spender.
        spender: String,
        /// Exact debit account.
        from: String,
        /// Exact credit account.
        to: String,
        /// Exact raw units.
        amount: String,
    },
}
impl ContractAction {
    pub(super) fn parse(intent: &UnsignedContractIntent) -> Result<Self, QualificationError> {
        if intent.value_wei() != "0" || intent.call().outputs() != ["bool"] {
            return Err(QualificationError::InvalidIntent);
        }
        let args = intent.call().arguments();
        let word = |index: usize| {
            args.get(index)
                .and_then(serde_json::Value::as_str)
                .ok_or(QualificationError::InvalidIntent)
        };
        let address = |index| parse_address(word(index)?).map(|v| format!("{v:#x}"));
        let amount = |index| {
            word(index)?
                .parse::<U256>()
                .map(|v| v.to_string())
                .map_err(|_| QualificationError::InvalidIntent)
        };
        match intent.call().signature() {
            "approve(address,uint256)" if args.len() == 2 => Ok(Self::Approve {
                owner: intent.account().into(),
                spender: address(0)?,
                amount: amount(1)?,
            }),
            "transfer(address,uint256)" if args.len() == 2 => Ok(Self::Transfer {
                from: intent.account().into(),
                to: address(0)?,
                amount: amount(1)?,
            }),
            "transferFrom(address,address,uint256)" if args.len() == 3 => Ok(Self::TransferFrom {
                spender: intent.account().into(),
                from: address(0)?,
                to: address(1)?,
                amount: amount(2)?,
            }),
            _ => Err(QualificationError::InvalidIntent),
        }
    }
    pub(super) fn queries(&self, target: Address) -> Result<Vec<StateQuery>, QualificationError> {
        let mut keys = BTreeSet::new();
        match self {
            Self::Approve { owner, spender, .. } => {
                keys.insert(allowance_slot(
                    parse_address(owner)?,
                    parse_address(spender)?,
                ));
            }
            Self::Transfer { from, to, .. } => {
                keys.insert(balance_slot(parse_address(from)?));
                keys.insert(balance_slot(parse_address(to)?));
            }
            Self::TransferFrom {
                spender, from, to, ..
            } => {
                keys.insert(allowance_slot(
                    parse_address(from)?,
                    parse_address(spender)?,
                ));
                keys.insert(balance_slot(parse_address(from)?));
                keys.insert(balance_slot(parse_address(to)?));
            }
        }
        Ok(keys
            .into_iter()
            .map(|slot| StateQuery::Storage {
                address: format!("{target:#x}"),
                slot: format!("{slot:#066x}"),
            })
            .collect())
    }
}
pub(super) fn parse_address(value: &str) -> Result<Address, QualificationError> {
    let value = Address::from_str(value).map_err(|_| QualificationError::InvalidIntent)?;
    if value == Address::ZERO {
        return Err(QualificationError::InvalidIntent);
    }
    Ok(value)
}
pub(super) fn balance_slot(account: Address) -> U256 {
    mapping_slot(account, U256::ZERO)
}
pub(super) fn allowance_slot(owner: Address, spender: Address) -> U256 {
    mapping_slot(spender, mapping_slot(owner, U256::from(1)))
}
fn mapping_slot(account: Address, base: U256) -> U256 {
    let mut bytes = [0u8; 64];
    bytes[12..32].copy_from_slice(account.as_slice());
    bytes[32..].copy_from_slice(&base.to_be_bytes::<32>());
    U256::from_be_bytes(keccak256(bytes).0)
}
