//! Compare fresh observations with the exact approved envelope without mutating it.
use crate::evm_approval::ApprovalCandidate;
use chain_evm::SimulationState;
use ipc_types::IpcError;

pub(crate) async fn revalidate(
    candidate: &ApprovalCandidate,
    policy: RefreshPolicy,
) -> Result<(), IpcError> {
    let network = candidate.network()?;
    let fresh = match candidate {
        ApprovalCandidate::Native(old) => ApprovalCandidate::Native(
            super::native(network, old.sender(), old.recipient(), old.amount_eth()).await?,
        ),
        ApprovalCandidate::Token(old) => ApprovalCandidate::Token(
            super::token(
                network,
                old.sender(),
                old.decoded().recipient(),
                old.decoded().amount(),
            )
            .await?,
        ),
    };
    if remains_valid(&facts(candidate), &facts(&fresh), policy) {
        Ok(())
    } else {
        Err(IpcError::invalid_request(
            "Chain facts changed or simulation did not succeed. A new transaction and fresh native approval are required",
        ))
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
    // Gas, fee cap, priority cap, total network bound, L1 bound, operator bound.
    limits: [&'a str; 6],
    native_balance: &'a str,
    token_balance: Option<&'a str>,
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
            limits: [
                c.gas_limit(),
                c.max_fee_per_gas_wei(),
                c.max_priority_fee_per_gas_wei(),
                c.maximum_network_fee_wei(),
                c.maximum_l1_fee_wei(),
                c.operator_fee_wei(),
            ],
            native_balance: c.account_balance_wei(),
            token_balance: None,
        },
        ApprovalCandidate::Token(c) => Facts {
            chain: c.chain_id(),
            sender: c.sender(),
            recipient: c.decoded().recipient(),
            amount: c.decoded().raw_amount(),
            data: c.data(),
            nonce: c.nonce(),
            simulation: c.simulation().state(),
            limits: [
                c.gas_limit(),
                c.max_fee_per_gas_wei(),
                c.max_priority_fee_per_gas_wei(),
                c.maximum_network_fee_wei(),
                c.maximum_l1_fee_wei(),
                c.operator_fee_wei(),
            ],
            native_balance: c.native_balance_wei(),
            token_balance: Some(c.token_balance_raw()),
        },
    }
}
fn remains_valid(old: &Facts<'_>, fresh: &Facts<'_>, policy: RefreshPolicy) -> bool {
    old.chain == fresh.chain
        && old.sender == fresh.sender
        && old.recipient == fresh.recipient
        && old.amount == fresh.amount
        && old.data == fresh.data
        && old.nonce == fresh.nonce
        && policy.accepts(old.simulation, fresh.simulation)
        && old
            .limits
            .iter()
            .zip(fresh.limits)
            .all(|(approved, current)| within(current, approved))
        && within(old.native_balance, fresh.native_balance)
        && match (old.token_balance, fresh.token_balance) {
            (None, None) => true,
            (Some(old), Some(fresh)) => within(old, fresh),
            _ => false,
        }
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
/// Selected only by trusted entry-point routing, never by model/IPC arguments.
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
            limits: ["100"; 6],
            native_balance: "1000",
            token_balance: None,
        }
    }
    #[test]
    fn every_network_asset_pair_retains_exact_intent_and_bounded_fees() {
        for chain in [1, 8_453, 84_532, 11_155_111] {
            for token in [false, true] {
                let mut old = original();
                old.chain = chain;
                old.token_balance = token.then_some("1000");
                assert!(remains_valid(&old, &old, RefreshPolicy::Chat));
                let mut fresh = old.clone();
                fresh.limits = ["99"; 6];
                assert!(remains_valid(&old, &fresh, RefreshPolicy::Chat));
                for index in 0..6 {
                    let mut fresh = old.clone();
                    fresh.limits[index] = "101";
                    assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                }
                let mut fresh = old.clone();
                fresh.nonce = "5";
                assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                let mut fresh = old.clone();
                fresh.chain = 42;
                assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                let mut fresh = old.clone();
                fresh.sender = "other";
                assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                let mut fresh = old.clone();
                fresh.recipient = "other";
                assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                let mut fresh = old.clone();
                fresh.amount = "101";
                assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                let mut fresh = old.clone();
                fresh.data = "0x095ea7b3";
                assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                let mut fresh = old.clone();
                fresh.native_balance = "999";
                assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                if token {
                    let mut fresh = old.clone();
                    fresh.token_balance = Some("999");
                    assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                }
                for state in [
                    SimulationState::Reverted,
                    SimulationState::Mismatch,
                    SimulationState::Unavailable,
                    SimulationState::Inconclusive,
                ] {
                    let mut fresh = old.clone();
                    fresh.simulation = state;
                    assert!(!remains_valid(&old, &fresh, RefreshPolicy::Chat));
                }
            }
        }
    }
    #[test]
    fn fees_use_exact_integer_arithmetic_and_reject_malformed_observations() {
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
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn same_inconclusive_candidate_preserves_wallet_warning_but_is_not_chat_eligible() {
        assert!(
            RefreshPolicy::Wallet
                .accepts(SimulationState::Inconclusive, SimulationState::Inconclusive)
        );
        assert!(
            !RefreshPolicy::Chat
                .accepts(SimulationState::Inconclusive, SimulationState::Inconclusive)
        );
        for state in [SimulationState::Unavailable, SimulationState::Reverted] {
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
}
