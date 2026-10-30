//! Check current requirements against the immutable reviewed envelope and budget.
use crate::evm_approval::ApprovalCandidate;
use chain_evm::{SimulationState, TransferFeeBudget, execution_fee_cap_covers};
use ipc_types::IpcError;

pub(crate) async fn revalidate(
    candidate: &ApprovalCandidate,
    policy: RefreshPolicy,
    budget: &TransferFeeBudget,
) -> Result<(), IpcError> {
    let network = candidate.network()?;
    let endpoint = crate::evm_rpc_settings::resolved(network)?;
    let fresh = match candidate {
        ApprovalCandidate::Native(old) => {
            let old = old.clone();
            let fresh = tokio::task::spawn_blocking(move || {
                chain_evm::refresh_native_transfer(network, &endpoint, &old)
            })
            .await
            .map_err(|_| IpcError::internal())?
            .map_err(|e| super::super::transfer_error(&e))?;
            ApprovalCandidate::Native(fresh)
        }
        ApprovalCandidate::Token(old) => {
            let old = old.clone();
            let fresh = tokio::task::spawn_blocking(move || {
                chain_evm::refresh_token_transfer(network, &endpoint, &old)
            })
            .await
            .map_err(|_| IpcError::internal())?
            .map_err(|e| super::super::token_error(&e))?;
            ApprovalCandidate::Token(fresh)
        }
    };
    if let Some(reason) = rejection(&facts(candidate), &facts(&fresh), policy, budget) {
        Err(IpcError::invalid_request(format!(
            "Transfer freshness check failed: {reason}. The approved transaction and fee budget were not increased. Prepare a new transaction for fresh native approval."
        )))
    } else {
        Ok(())
    }
}
#[derive(Clone)]
struct Facts<'a> {
    chain: u64,
    sender: &'a str,
    recipient: &'a str,
    amount: &'a str,
    data: &'a str,
    nonce: &'a str,
    simulation: SimulationState,
    gas: &'a str,
    base: &'a str,
    cap: &'a str,
    priority: &'a str,
    l1: &'a str,
    operator: &'a str,
    native_balance: &'a str,
    token_balance: Option<&'a str>,
    value: &'a str,
}
fn facts(candidate: &ApprovalCandidate) -> Facts<'_> {
    match candidate {
        ApprovalCandidate::Native(c) => Facts {
            chain: c.chain_id(),
            sender: c.sender(),
            recipient: c.recipient(),
            amount: c.value_wei(),
            data: c.data(),
            nonce: c.nonce(),
            simulation: c.simulation().state(),
            gas: c.gas_limit(),
            base: c.base_fee_per_gas_wei(),
            cap: c.max_fee_per_gas_wei(),
            priority: c.max_priority_fee_per_gas_wei(),
            l1: c.maximum_l1_fee_wei(),
            operator: c.operator_fee_wei(),
            native_balance: c.account_balance_wei(),
            token_balance: None,
            value: c.value_wei(),
        },
        ApprovalCandidate::Token(c) => Facts {
            chain: c.chain_id(),
            sender: c.sender(),
            recipient: c.decoded().recipient(),
            amount: c.decoded().raw_amount(),
            data: c.data(),
            nonce: c.nonce(),
            simulation: c.simulation().state(),
            gas: c.gas_limit(),
            base: c.base_fee_per_gas_wei(),
            cap: c.max_fee_per_gas_wei(),
            priority: c.max_priority_fee_per_gas_wei(),
            l1: c.maximum_l1_fee_wei(),
            operator: c.operator_fee_wei(),
            native_balance: c.native_balance_wei(),
            token_balance: Some(c.token_balance_raw()),
            value: "0",
        },
    }
}
fn rejection(
    old: &Facts<'_>,
    fresh: &Facts<'_>,
    policy: RefreshPolicy,
    budget: &TransferFeeBudget,
) -> Option<&'static str> {
    if old.chain != fresh.chain {
        return Some("chain_changed");
    }
    if old.sender != fresh.sender
        || old.recipient != fresh.recipient
        || old.amount != fresh.amount
        || old.data != fresh.data
    {
        return Some("transfer_intent_changed");
    }
    if old.nonce != fresh.nonce {
        return Some("nonce_changed");
    }
    if !policy.accepts(old.simulation, fresh.simulation) {
        return Some("simulation_not_acceptable");
    }
    if !within(fresh.gas, old.gas) {
        return Some("gas_limit_exceeded");
    }
    if old.cap != fresh.cap || old.priority != fresh.priority {
        return Some("signed_execution_fee_fields_changed");
    }
    if !execution_fee_cap_covers(fresh.base, old.priority, old.cap).unwrap_or(false) {
        return Some("base_fee_exceeds_signed_cap");
    }
    if !within(fresh.l1, budget.l1_wei()) {
        return Some("l1_fee_budget_exceeded");
    }
    if !within(fresh.operator, budget.operator_wei()) {
        return Some("operator_fee_budget_exceeded");
    }
    if !budget
        .covers_rollup_quotes(fresh.l1, fresh.operator)
        .unwrap_or(false)
    {
        return Some("combined_fee_budget_exceeded");
    }
    if !within(old.native_balance, fresh.native_balance) {
        return Some("native_balance_reduced");
    }
    if !budget
        .affordable(fresh.native_balance, old.value)
        .unwrap_or(false)
    {
        return Some("fee_budget_unaffordable");
    }
    if !match (old.token_balance, fresh.token_balance) {
        (None, None) => true,
        (Some(old), Some(fresh)) => within(old, fresh),
        _ => false,
    } {
        return Some("token_balance_reduced");
    }
    None
}
fn within(current: &str, approved: &str) -> bool {
    if current.is_empty()
        || approved.is_empty()
        || !current
            .bytes()
            .chain(approved.bytes())
            .all(|c| c.is_ascii_digit())
    {
        return false;
    }
    let current = current.trim_start_matches('0');
    let approved = approved.trim_start_matches('0');
    (current.len(), current) <= (approved.len(), approved)
}
/// Only trusted host routing chooses this; model and IPC arguments cannot.
#[derive(Clone, Copy)]
pub(crate) enum RefreshPolicy {
    Wallet,
    Chat,
}
impl RefreshPolicy {
    pub(crate) fn accepts(self, approved: SimulationState, fresh: SimulationState) -> bool {
        if approved == SimulationState::Mismatch || fresh == SimulationState::Mismatch {
            return false;
        }
        match self {
            Self::Chat => {
                approved == SimulationState::Succeeded && fresh == SimulationState::Succeeded
            }
            Self::Wallet => fresh == SimulationState::Succeeded || fresh == approved,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn original() -> Facts<'static> {
        Facts {
            chain: 8453,
            sender: "sender",
            recipient: "recipient",
            amount: "100",
            data: "0x",
            nonce: "4",
            simulation: SimulationState::Succeeded,
            gas: "100",
            base: "40",
            cap: "100",
            priority: "10",
            l1: "100",
            operator: "100",
            native_balance: "20000",
            token_balance: None,
            value: "100",
        }
    }
    fn budget() -> Result<TransferFeeBudget, chain_evm::FeeBudgetError> {
        TransferFeeBudget::new("10000", "100", "100")
    }
    #[test]
    fn fresh_raw_quotes_can_move_within_frozen_headroom_without_rebuffering()
    -> Result<(), chain_evm::FeeBudgetError> {
        let old = original();
        let budget = budget()?;
        let mut fresh = old.clone();
        fresh.l1 = "150";
        fresh.operator = "149";
        fresh.base = "90";
        assert_eq!(rejection(&old, &fresh, RefreshPolicy::Chat, &budget), None);
        assert_eq!(budget.l1_wei(), "150");
        fresh.l1 = "151";
        assert_eq!(
            rejection(&old, &fresh, RefreshPolicy::Chat, &budget),
            Some("l1_fee_budget_exceeded")
        );
        fresh.l1 = "150";
        fresh.operator = "151";
        assert_eq!(
            rejection(&old, &fresh, RefreshPolicy::Chat, &budget),
            Some("operator_fee_budget_exceeded")
        );
        fresh.operator = "150";
        fresh.base = "91";
        assert_eq!(
            rejection(&old, &fresh, RefreshPolicy::Chat, &budget),
            Some("base_fee_exceeds_signed_cap")
        );
        Ok(())
    }
    #[test]
    fn all_networks_and_assets_keep_exact_intent_balance_and_simulation_checks()
    -> Result<(), chain_evm::FeeBudgetError> {
        let frozen_budget = budget()?;
        for chain in [1, 8_453, 84_532, 11_155_111] {
            for token in [false, true] {
                let mut old = original();
                old.chain = chain;
                old.token_balance = token.then_some("1000");
                assert_eq!(
                    rejection(&old, &old, RefreshPolicy::Chat, &frozen_budget),
                    None
                );
                for (field, reason) in [
                    ("chain", "chain_changed"),
                    ("sender", "transfer_intent_changed"),
                    ("recipient", "transfer_intent_changed"),
                    ("amount", "transfer_intent_changed"),
                    ("data", "transfer_intent_changed"),
                    ("nonce", "nonce_changed"),
                    ("gas", "gas_limit_exceeded"),
                    ("cap", "signed_execution_fee_fields_changed"),
                    ("priority", "signed_execution_fee_fields_changed"),
                    ("balance", "native_balance_reduced"),
                ] {
                    let mut fresh = old.clone();
                    match field {
                        "chain" => fresh.chain = 42,
                        "sender" => fresh.sender = "other",
                        "recipient" => fresh.recipient = "other",
                        "amount" => fresh.amount = "101",
                        "data" => fresh.data = "0x01",
                        "nonce" => fresh.nonce = "5",
                        "gas" => fresh.gas = "101",
                        "cap" => fresh.cap = "101",
                        "priority" => fresh.priority = "11",
                        _ => fresh.native_balance = "19999",
                    }
                    assert_eq!(
                        rejection(&old, &fresh, RefreshPolicy::Chat, &frozen_budget),
                        Some(reason)
                    );
                }
                if token {
                    let mut fresh = old.clone();
                    fresh.token_balance = Some("999");
                    assert_eq!(
                        rejection(&old, &fresh, RefreshPolicy::Chat, &frozen_budget),
                        Some("token_balance_reduced")
                    );
                }
                for state in [
                    SimulationState::Reverted,
                    SimulationState::Mismatch,
                    SimulationState::Unavailable,
                    SimulationState::Inconclusive,
                ] {
                    let mut fresh = old.clone();
                    fresh.simulation = state;
                    assert_eq!(
                        rejection(&old, &fresh, RefreshPolicy::Chat, &frozen_budget),
                        Some("simulation_not_acceptable")
                    );
                }
            }
        }
        Ok(())
    }
    #[test]
    fn wallet_keeps_reviewed_warning_policy_while_chat_remains_strict() {
        for state in [
            SimulationState::Inconclusive,
            SimulationState::Unavailable,
            SimulationState::Reverted,
        ] {
            assert!(RefreshPolicy::Wallet.accepts(state, state));
            assert!(!RefreshPolicy::Chat.accepts(state, state));
        }
        assert!(
            !RefreshPolicy::Wallet
                .accepts(SimulationState::Succeeded, SimulationState::Inconclusive)
        );
        assert!(
            !RefreshPolicy::Wallet.accepts(SimulationState::Mismatch, SimulationState::Succeeded)
        );
    }
    #[test]
    fn comparisons_are_exact_and_malformed_or_overflowing_observations_fail_closed()
    -> Result<(), chain_evm::FeeBudgetError> {
        let frozen_budget = budget()?;
        assert!(within(
            "99999999999999999999999999999",
            "100000000000000000000000000000"
        ));
        assert!(!within(
            "100000000000000000000000000001",
            "100000000000000000000000000000"
        ));
        assert!(within("000", "0"));
        for invalid in ["", "-1", "1.0", "1e9", "0x10"] {
            assert!(!within(invalid, "100"));
        }
        let old = original();
        let mut fresh = old.clone();
        fresh.l1 = "1e3";
        assert_eq!(
            rejection(&old, &fresh, RefreshPolicy::Chat, &frozen_budget),
            Some("l1_fee_budget_exceeded")
        );
        Ok(())
    }
}
