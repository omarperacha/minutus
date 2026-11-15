//! Exact-root reads borrow sealed replay evidence; missing facts remain proof IO.
use super::completion_diagnostics::{self as diagnostics, Stage};
use super::{GenericContractCandidate, GenericError, GenericLiveCohort};
use crate::{
    EvmNetwork, EvmRpcUrl,
    local_simulation::{self, ProvedReader, ProvedState},
};
use alloy_primitives::{Address, B256, U256};
use evm_simulation::{SimulationControl, SimulationNetwork, StateQuery, StateValue};
use std::str::FromStr;

pub(super) struct CandidateStateReader<'a> {
    endpoint: &'a EvmRpcUrl,
    network: EvmNetwork,
    hash: &'a str,
    number: u64,
    root: B256,
    before: Option<&'a ProvedState>,
    missing: Option<ProvedReader<'a>>,
    queries: Vec<StateQuery>,
}
impl<'a> CandidateStateReader<'a> {
    pub(super) fn at_cohort(
        endpoint: &'a EvmRpcUrl,
        cohort: &'a GenericLiveCohort,
        control: &SimulationControl<'_>,
    ) -> Result<Self, GenericError> {
        let reader = diagnostics::simulation(Stage::ConditionReaderSetup, control, || {
            ProvedReader::new(endpoint, cohort.network(), cohort.block_hash(), control)
        })
        .map_err(state_error)?;
        if reader.block_number() != cohort.block_number() {
            return Err(GenericError::SnapshotMismatch);
        }
        Ok(Self {
            endpoint,
            network: cohort.network(),
            hash: cohort.block_hash(),
            number: cohort.block_number(),
            root: reader.state_root(),
            before: None,
            missing: Some(reader),
            queries: vec![],
        })
    }
    pub(super) fn from_candidate(
        endpoint: &'a EvmRpcUrl,
        candidate: &'a GenericContractCandidate,
    ) -> Result<Self, GenericError> {
        let replay = candidate.replay();
        let network = match replay.request().network {
            SimulationNetwork::EthereumMainnet => EvmNetwork::EthereumMainnet,
            SimulationNetwork::EthereumSepolia => EvmNetwork::EthereumSepolia,
            SimulationNetwork::BaseMainnet => EvmNetwork::BaseMainnet,
            SimulationNetwork::BaseSepolia => EvmNetwork::BaseSepolia,
        };
        if network != candidate.intent().network() {
            return Err(GenericError::SnapshotMismatch);
        }
        // TrustedReplay privately joins this request's complete hash-checked
        // header/state root to its MPT-checked beforestate. No bare ProvedState
        // plus caller-supplied hash enters this constructor.
        Ok(Self {
            endpoint,
            network,
            hash: &replay.request().block.hash,
            number: replay.request().block.number,
            root: B256::from_str(&replay.request().block.state_root)
                .map_err(|_| GenericError::SnapshotMismatch)?,
            before: Some(replay.state()),
            missing: None,
            queries: vec![],
        })
    }
    pub(super) fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, GenericError> {
        control.remaining().map_err(state_error)?;
        if !self.queries.contains(query) {
            if self.queries.len() >= 128 {
                return Err(GenericError::Limit);
            }
            self.queries.push(query.clone());
        }
        if let Some(value) = self
            .before
            .map(|state| cached(state, query))
            .transpose()?
            .flatten()
        {
            return Ok(value);
        }
        if self.missing.is_none() {
            let reader = diagnostics::simulation(Stage::ConditionReaderSetup, control, || {
                ProvedReader::new(self.endpoint, self.network, self.hash, control)
            })
            .map_err(state_error)?;
            if reader.block_number() != self.number || reader.state_root() != self.root {
                return Err(GenericError::SnapshotMismatch);
            }
            self.missing = Some(reader);
        }
        diagnostics::simulation(Stage::ReaderQuery, control, || {
            self.missing
                .as_mut()
                .ok_or(evm_simulation::SimulationError::StateUnavailable)?
                .query(query, control)
        })
        .map_err(state_error)
    }
    pub(super) fn finish(
        self,
        control: &SimulationControl<'_>,
    ) -> Result<ProvedState, GenericError> {
        if self.missing.is_none() {
            let header = diagnostics::simulation(Stage::ReaderCanonicalFinish, control, || {
                local_simulation::canonical_header(self.endpoint, self.network, self.hash, control)
            })
            .map_err(state_error)?;
            if header.number != self.number || header.state_root != self.root {
                return Err(GenericError::SnapshotMismatch);
            }
        }
        self.collect(control)
    }
    // This releases only private proof state, never a condition certificate.
    // Its caller must finalise the retained cohort before exposing conditions.
    pub(super) fn finish_pending(
        self,
        cohort: &GenericLiveCohort,
        control: &SimulationControl<'_>,
    ) -> Result<ProvedState, GenericError> {
        if self.network != cohort.network()
            || self.hash != cohort.block_hash()
            || self.number != cohort.block_number()
        {
            return Err(GenericError::SnapshotMismatch);
        }
        self.collect(control)
    }
    fn collect(self, control: &SimulationControl<'_>) -> Result<ProvedState, GenericError> {
        let extra = self
            .missing
            .map(|reader| {
                diagnostics::simulation(Stage::ReaderCanonicalFinish, control, || {
                    reader.finish(control)
                })
            })
            .transpose()
            .map_err(state_error)?;
        match (self.before, extra) {
            (Some(before), Some(extra)) => {
                let mut state = before.clone();
                state.merge(extra).map_err(state_error)?;
                Ok(state)
            }
            (Some(before), None) => Ok(before.clone()),
            (None, Some(state)) => Ok(state),
            (None, None) => Err(GenericError::MissingState),
        }
    }
}
fn cached(state: &ProvedState, query: &StateQuery) -> Result<Option<StateValue>, GenericError> {
    match query {
        StateQuery::Account { address } => {
            let address = Address::from_str(address).map_err(|_| GenericError::InvalidAbi)?;
            Ok(state.account(address).map(|account| StateValue::Account {
                exists: account.exists(),
                balance: format!("{:#x}", account.balance()),
                nonce: account.nonce(),
                code: format!("0x{}", alloy_primitives::hex::encode(account.code())),
            }))
        }
        StateQuery::Storage { address, slot } => {
            let address = Address::from_str(address).map_err(|_| GenericError::InvalidAbi)?;
            let slot = U256::from_str(slot).map_err(|_| GenericError::InvalidAbi)?;
            Ok(state.slot(address, slot).map(|value| StateValue::Storage {
                value: format!("{value:#066x}"),
            }))
        }
        StateQuery::BlockHash { .. } => Err(GenericError::OpaqueEffects),
    }
}
fn state_error(error: evm_simulation::SimulationError) -> GenericError {
    match error {
        evm_simulation::SimulationError::Limit => GenericError::Limit,
        evm_simulation::SimulationError::Cancelled => GenericError::Expired,
        _ => GenericError::MissingState,
    }
}

pub(super) fn live_candidate_cohort(
    candidate: &GenericContractCandidate,
) -> Result<&GenericLiveCohort, GenericError> {
    let cohort = candidate
        .live_cohort
        .as_ref()
        .ok_or(GenericError::SnapshotMismatch)?;
    check_cohort(
        cohort,
        candidate.intent().network(),
        candidate.block_hash(),
        candidate.block_number(),
    )?;
    if !candidate.is_fresh() || !cohort.is_fresh() {
        return Err(GenericError::Expired);
    }
    Ok(cohort)
}
fn check_cohort(
    cohort: &GenericLiveCohort,
    network: EvmNetwork,
    hash: &str,
    number: u64,
) -> Result<(), GenericError> {
    if cohort.network() != network || cohort.block_hash() != hash || cohort.block_number() != number
    {
        return Err(GenericError::SnapshotMismatch);
    }
    Ok(())
}
#[cfg(test)]
mod tests;
