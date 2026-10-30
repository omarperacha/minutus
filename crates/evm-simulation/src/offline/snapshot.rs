use crate::{
    MAX_STATE_BYTES, MAX_STATE_QUERIES, SimulationControl, SimulationError, SimulationRequest,
    SimulationStateSource, StateQuery, StateValue, admission, protocol,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fact {
    pub(super) query: StateQuery,
    pub(super) value: StateValue,
}

/// Frozen non-secret state for one unsigned request, prepared before signature
/// material exists. The trusted host source owns proof/header/canonical checks.
/// This type grants no signing or submission authority.
pub struct FrozenBaseRequest {
    pub(super) request: SimulationRequest,
    pub(super) facts: Vec<Fact>,
    pub(super) commitment: String,
}
impl FrozenBaseRequest {
    /// Acquires exactly a bounded, host-selected closed state manifest. Neither
    /// the manifest nor source may come from a model's assertion of verification.
    /// The source must verify every answer against the request's header/state root.
    /// Execution later cannot fetch additional state or enlarge this manifest.
    ///
    /// # Errors
    /// Rejects unsupported contexts, aliases/duplicates, unavailable or malformed
    /// state, resource limits and cancellation. No signed bytes are accepted here.
    pub fn prepare(
        request: SimulationRequest,
        manifest: &[StateQuery],
        source: &mut impl SimulationStateSource,
        control: &SimulationControl<'_>,
    ) -> Result<Self, SimulationError> {
        validate_request(&request)?;
        if manifest.is_empty() || manifest.len() > MAX_STATE_QUERIES as usize {
            return Err(SimulationError::Limit);
        }
        let mut facts = Vec::new();
        let mut keys = BTreeSet::new();
        let mut total = 0;
        for query in manifest {
            control.remaining()?;
            admission::validate_query(query, request.block.number)?;
            if !keys.insert(key(query)?) {
                return Err(SimulationError::InvalidInput);
            }
            let value = source.query(query, control)?;
            control.remaining()?;
            let fact = Fact {
                query: query.clone(),
                value,
            };
            total += serde_json::to_vec(&fact)
                .map_err(|_| SimulationError::Protocol)?
                .len();
            if total > MAX_STATE_BYTES {
                return Err(SimulationError::Limit);
            }
            facts.push(fact);
        }
        Self::from_facts(request, facts)
    }
    pub(super) fn from_facts(
        request: SimulationRequest,
        mut facts: Vec<Fact>,
    ) -> Result<Self, SimulationError> {
        validate_request(&request)?;
        if facts.is_empty() || facts.len() > MAX_STATE_QUERIES as usize {
            return Err(SimulationError::Limit);
        }
        let mut keyed = facts
            .into_iter()
            .map(|fact| Ok((key(&fact.query)?, fact)))
            .collect::<Result<Vec<_>, SimulationError>>()?;
        keyed.sort_by(|left, right| left.0.cmp(&right.0));
        facts = keyed.into_iter().map(|(_, fact)| fact).collect();
        let commitment = commitment(&request, &facts)?;
        let result = Self {
            request,
            facts,
            commitment,
        };
        let _ = super::database::FrozenDb::new(&result)?;
        Ok(result)
    }
    /// Commitment to the exact unsigned request and complete frozen state.
    #[must_use]
    pub fn commitment(&self) -> &str {
        &self.commitment
    }
    /// A rigorous all-signature fee ceiling at this proved snapshot, not a
    /// guarantee about changed inclusion-time L1 parameters.
    ///
    /// # Errors
    /// Rejects missing fee facts, unsupported rules and arithmetic overflow.
    pub fn fee_ceiling(&self) -> Result<super::BaseFeeCeiling, SimulationError> {
        super::fees::ceiling(self)
    }
}
pub(super) fn key(query: &StateQuery) -> Result<String, SimulationError> {
    serde_json::to_string(query)
        .map(|value| value.to_ascii_lowercase())
        .map_err(|_| SimulationError::Protocol)
}
pub(super) fn validate_request(request: &SimulationRequest) -> Result<(), SimulationError> {
    if admission::quantity(&request.transaction.value)? != alloy_primitives::U256::ZERO {
        return Err(SimulationError::UnsupportedExecution);
    }
    match admission::validate(request)? {
        admission::Rules::BaseCobalt | admission::Rules::BaseDenim => Ok(()),
        _ => Err(SimulationError::UnsupportedRules),
    }
}
fn commitment(request: &SimulationRequest, facts: &[Fact]) -> Result<String, SimulationError> {
    let request = protocol::commitment(request)?;
    let facts = serde_json::to_vec(facts).map_err(|_| SimulationError::Protocol)?;
    if facts.len() > MAX_STATE_BYTES {
        return Err(SimulationError::Limit);
    }
    let mut digest = Sha256::new();
    digest.update(b"minutus/frozen-base/v1\0");
    digest.update(request.as_bytes());
    digest.update(&facts);
    Ok(format!("{:x}", digest.finalize()))
}

/// Fixed native account/fee-slot reads for admitted direct zero-value Base calls.
/// Append the source profile's complete action slots before preparing the snapshot.
/// Activation-registry reads are not implicit: the direct ERC20 family cannot
/// call that precompile. Any extra read after signing remains a terminal failure.
///
/// # Errors
/// Rejects unsupported request rules and native-value scope.
pub fn base_native_manifest(
    request: &SimulationRequest,
) -> Result<Vec<StateQuery>, SimulationError> {
    validate_request(request)?;
    let mut accounts = BTreeSet::new();
    for address in [
        &request.transaction.caller,
        &request.transaction.target,
        &request.block.beneficiary,
        "0x4200000000000000000000000000000000000015",
        "0x4200000000000000000000000000000000000019",
        "0x420000000000000000000000000000000000001a",
        "0x420000000000000000000000000000000000001b",
    ] {
        accounts.insert(format!("{:#x}", admission::address(address)?));
    }
    let mut manifest: Vec<_> = accounts
        .into_iter()
        .map(|address| StateQuery::Account { address })
        .collect();
    for slot in [1, 3, 7, 8] {
        manifest.push(StateQuery::Storage {
            address: "0x4200000000000000000000000000000000000015".into(),
            slot: format!("0x{slot:064x}"),
        });
    }
    Ok(manifest)
}
