//! Fixed pre-submit fee budgets, distinct from changing oracle observations.

use crate::rpc::{Uint256, format_units};
use serde::Serialize;
use std::{error::Error, fmt};

/// Reviewed execution cap plus 50% headroom for Base L1/operator quotes.
/// This is a pre-submit budget, not an on-chain cap on future rollup fees.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferFeeBudget {
    #[serde(rename = "executionWei")]
    execution: String,
    #[serde(rename = "l1Wei")]
    l1: String,
    #[serde(rename = "operatorWei")]
    operator: String,
    #[serde(rename = "totalWei")]
    total: String,
}
impl TransferFeeBudget {
    /// Builds a budget once, before approval, using exact rounded-up arithmetic.
    /// # Errors
    /// Rejects malformed quotes or any uint256 arithmetic overflow.
    pub fn new(execution: &str, l1: &str, operator: &str) -> Result<Self, FeeBudgetError> {
        let execution = parse(execution)?;
        let l1 = headroom(parse(l1)?)?;
        let operator = headroom(parse(operator)?)?;
        let total = execution
            .checked_add(l1)
            .and_then(|v| v.checked_add(operator))
            .ok_or(FeeBudgetError)?;
        Ok(Self {
            execution: execution.to_decimal(),
            l1: l1.to_decimal(),
            operator: operator.to_decimal(),
            total: total.to_decimal(),
        })
    }
    /// Approved L1 quote budget, in wei.
    #[must_use]
    pub fn l1_wei(&self) -> &str {
        &self.l1
    }
    /// Approved operator quote budget, in wei.
    #[must_use]
    pub fn operator_wei(&self) -> &str {
        &self.operator
    }
    /// Approved combined pre-submit budget, in wei.
    #[must_use]
    pub fn total_wei(&self) -> &str {
        &self.total
    }
    /// Approved combined pre-submit budget, in ETH.
    #[must_use]
    pub fn total_eth(&self) -> String {
        format_units(&self.total, 18)
    }
    /// Checks fresh unbuffered quotes against this frozen budget.
    /// # Errors
    /// Rejects malformed observations or arithmetic overflow.
    pub fn covers_rollup_quotes(&self, l1: &str, operator: &str) -> Result<bool, FeeBudgetError> {
        let l1 = parse(l1)?;
        let operator = parse(operator)?;
        let total = parse(&self.execution)?
            .checked_add(l1)
            .and_then(|v| v.checked_add(operator))
            .ok_or(FeeBudgetError)?;
        Ok(l1 <= parse(&self.l1)?
            && operator <= parse(&self.operator)?
            && total <= parse(&self.total)?)
    }
    /// Checks that the current balance covers value and the full reviewed budget.
    /// # Errors
    /// Rejects malformed values or arithmetic overflow.
    pub fn affordable(&self, balance: &str, value: &str) -> Result<bool, FeeBudgetError> {
        let required = parse(&self.total)?
            .checked_add(parse(value)?)
            .ok_or(FeeBudgetError)?;
        Ok(required <= parse(balance)?)
    }
}
/// Whether current base fee plus the fixed approved tip fits the signed fee cap.
/// # Errors
/// Rejects malformed values or addition overflow. No cap or tip is changed.
pub fn execution_fee_cap_covers(
    base: &str,
    priority: &str,
    cap: &str,
) -> Result<bool, FeeBudgetError> {
    let required = parse(base)?
        .checked_add(parse(priority)?)
        .ok_or(FeeBudgetError)?;
    Ok(required <= parse(cap)?)
}
fn parse(value: &str) -> Result<Uint256, FeeBudgetError> {
    Uint256::from_decimal(value).map_err(|_| FeeBudgetError)
}
fn headroom(value: Uint256) -> Result<Uint256, FeeBudgetError> {
    let (half, odd) = value.div_small(2);
    value
        .checked_add(half)
        .and_then(|v| v.checked_add(Uint256::from_u64(u64::from(odd))))
        .ok_or(FeeBudgetError)
}
/// Invalid/overflowing fee quote or observation; no budget is produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeeBudgetError;
impl fmt::Display for FeeBudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invalid or overflowing transfer fee budget")
    }
}
impl Error for FeeBudgetError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headroom_is_exact_rounded_up_and_not_applied_again_at_refresh() -> Result<(), FeeBudgetError>
    {
        let budget = TransferFeeBudget::new("499697000000", "752", "1")?;
        assert_eq!(budget.l1_wei(), "1128");
        assert_eq!(budget.operator_wei(), "2");
        assert_eq!(budget.total_wei(), "499697001130");
        for quote in ["752", "832", "913", "1128"] {
            assert!(budget.covers_rollup_quotes(quote, "2")?);
        }
        assert!(!budget.covers_rollup_quotes("1129", "2")?);
        assert!(!budget.covers_rollup_quotes("913", "3")?);
        assert_eq!(budget.l1_wei(), "1128");
        assert!(!budget.affordable("499697001129", "0")?);
        assert!(budget.affordable("499697001131", "1")?);
        Ok(())
    }
    #[test]
    fn zero_ethereum_rollup_fees_remain_zero_and_invalid_or_overflowing_quotes_fail()
    -> Result<(), FeeBudgetError> {
        let budget = TransferFeeBudget::new("100", "0", "0")?;
        assert_eq!(budget.total_wei(), "100");
        assert!(!budget.covers_rollup_quotes("1", "0")?);
        for invalid in ["", "-1", "1.0", "0x10", "1e3"] {
            assert!(TransferFeeBudget::new("100", invalid, "0").is_err());
        }
        let maximum =
            "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert!(TransferFeeBudget::new("0", maximum, "0").is_err());
        assert!(TransferFeeBudget::new(maximum, "1", "0").is_err());
        assert!(execution_fee_cap_covers("5000000", "1000000", "11000000")?);
        assert!(execution_fee_cap_covers("10000000", "1000000", "11000000")?);
        assert!(!execution_fee_cap_covers(
            "10000001", "1000000", "11000000"
        )?);
        assert!(execution_fee_cap_covers(maximum, "1", maximum).is_err());
        Ok(())
    }
}
