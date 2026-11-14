//! Fixed, authority-free child protocol values and admission limits.
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

/// Maximum EVM gas admitted by the local preview engine.
pub const MAX_GAS: u64 = 1_000_000;
/// Maximum sequential state queries, including implicit precompile/account reads.
pub const MAX_STATE_QUERIES: u32 = 128;
/// Maximum opcode steps, independently of gas accounting.
pub const MAX_STEPS: u64 = 200_000;
/// Maximum call nesting admitted for reviewable traces.
pub const MAX_DEPTH: usize = 16;
/// Maximum frame or complete result size on the child pipe.
pub const MAX_FRAME: usize = 64 * 1024;
/// Maximum aggregate state responses admitted per run.
pub const MAX_STATE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum interpreter memory per context, supplemented by the process cap.
pub const MAX_EVM_MEMORY: u64 = 4 * 1024 * 1024;

/// Fixed sanitised failure. No provider messages, URLs, private state or secrets.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SimulationError {
    /// Malformed request or state bytes.
    InvalidInput,
    /// Unknown, expired or not yet qualified chain execution rules.
    UnsupportedRules,
    /// Fixed resource bound reached; partial output cannot qualify effects.
    Limit,
    /// Cancelled before completion; no usable result.
    Cancelled,
    /// Required exact-block state is unavailable.
    StateUnavailable,
    /// Hash, code, nonce or canonical block observations disagree.
    InconsistentState,
    /// Helper launch, isolation, resource accounting or exit failed.
    WorkerUnavailable,
    /// Base native fee validation requires an independently bound full envelope.
    BaseEnvelopeRequired,
    /// Exact transaction validation failed, without relaxed fee/balance checks.
    TransactionRejected,
    /// Unsupported code/account control, creation or destruction was encountered.
    UnsupportedExecution,
    /// Process protocol diverged or returned truncated/malformed data.
    Protocol,
}
impl fmt::Display for SimulationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Local EVM simulation failed: {self:?}")
    }
}
impl Error for SimulationError {}

/// Four existing network identities; never an arbitrary chain or model URL.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SimulationNetwork {
    /// Ethereum mainnet.
    EthereumMainnet,
    /// Ethereum Sepolia.
    EthereumSepolia,
    /// Base mainnet.
    BaseMainnet,
    /// Base Sepolia.
    BaseSepolia,
}
impl SimulationNetwork {
    /// Exact EIP-155 identity.
    #[must_use]
    pub const fn chain_id(self) -> u64 {
        match self {
            Self::EthereumMainnet => 1,
            Self::EthereumSepolia => 11_155_111,
            Self::BaseMainnet => 8453,
            Self::BaseSepolia => 84532,
        }
    }
}
/// Full public block execution context, supplied from one canonical hash.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimulationBlock {
    /// Exact canonical block identity.
    pub hash: String,
    /// Canonical state root; parent state acquisition binds to this block.
    pub state_root: String,
    /// Block height.
    pub number: u64,
    /// Unix timestamp in seconds; determines rules, never a caller-selected fork.
    pub timestamp: u64,
    /// Fee recipient address.
    pub beneficiary: String,
    /// Execution gas limit.
    pub gas_limit: u64,
    /// Base fee, exact wei.
    pub base_fee: u64,
    /// Beacon randomness, not a zero-filled default.
    pub prevrandao: String,
    /// Actual header excess blob gas.
    pub excess_blob_gas: u64,
    /// Exact consensus slot from Amsterdam header; never inferred or zero-filled.
    pub slot_number: Option<u64>,
    /// Exact Amsterdam block-access-list commitment from the same header.
    pub block_access_list_hash: Option<String>,
}
/// Unsigned ordinary type-2 execution facts; no signature/auth-list/deposit mode.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimulationTransaction {
    /// Exact public sender, not proof of account control.
    pub caller: String,
    /// Exact nonzero application target; deployment is excluded.
    pub target: String,
    /// Canonical hexadecimal wei quantity.
    pub value: String,
    /// Exact bounded call bytes.
    pub calldata: String,
    /// Host-observed account nonce, never skipped during validation.
    pub nonce: u64,
    /// Host-admitted gas limit.
    pub gas_limit: u64,
    /// Exact fee cap in wei.
    #[serde(with = "u128_string")]
    pub max_fee_per_gas: u128,
    /// Exact priority cap in wei.
    #[serde(with = "u128_string")]
    pub priority_fee_per_gas: u128,
}
/// Immutable one-shot request; validation occurs in both parent and worker.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimulationRequest {
    /// Existing network identity.
    pub network: SimulationNetwork,
    /// One exact canonical block snapshot.
    pub block: SimulationBlock,
    /// One exact unsigned ordinary transaction.
    pub transaction: SimulationTransaction,
}
/// Required state operation. No endpoint, filesystem selector or transaction send.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StateQuery {
    /// Account existence, nonce, balance and complete code at the pinned state.
    Account {
        /// Exact address.
        address: String,
    },
    /// Exact storage slot at the same pinned state.
    Storage {
        /// Account address.
        address: String,
        /// Exact 32-byte slot.
        slot: String,
    },
    /// Hash from the pinned block's verified ancestor chain.
    BlockHash {
        /// Ancestor height.
        number: u64,
    },
}
/// Bounded state response. Missing account is explicit, never a missing-data zero.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StateValue {
    /// Complete observed account or proved absence.
    Account {
        /// Whether the account exists at this state.
        exists: bool,
        /// Exact canonical wei quantity.
        balance: String,
        /// Account nonce.
        nonce: u64,
        /// Complete runtime bytes.
        code: String,
    },
    /// Exact slot value; availability/consistency must be checked by the parent.
    Storage {
        /// Complete 32-byte value.
        value: String,
    },
    /// Complete canonical ancestor hash.
    BlockHash {
        /// Hash bytes.
        hash: String,
    },
}
/// Non-reverting execution is separate from semantic task success.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionStatus {
    /// Non-reverting local execution; not task success.
    Succeeded,
    /// Reverted with bounded output bytes.
    Reverted,
    /// Halted without successful execution.
    Halted,
}
/// One exact call-frame observation; labels/ABI semantics are absent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallObservation {
    /// Actual storage/execution target.
    pub target: String,
    /// Actual code address (can differ for delegate dispatch).
    pub code_address: String,
    /// Actual caller.
    pub caller: String,
    /// Value at this frame, as canonical hexadecimal quantity.
    pub value: String,
    /// Call nesting depth.
    pub depth: usize,
}
/// Changed storage word; no inference of asset or permission meaning.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageChange {
    /// Exact account.
    pub address: String,
    /// Exact key.
    pub slot: String,
    /// Original value.
    pub before: String,
    /// Local simulated value.
    pub after: String,
}
/// One changed account, including execution fee/nonce effects.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountChange {
    /// Exact account.
    pub address: String,
    /// Original proved native balance.
    pub balance_before: String,
    /// Final simulated balance; not a universal token balance.
    pub balance: String,
    /// Original proved nonce.
    pub nonce_before: u64,
    /// Final simulated nonce.
    pub nonce: u64,
}
/// One emitted log from non-reverting simulated execution; untrusted claims.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogObservation {
    /// Actual emitter address.
    pub address: String,
    /// Complete ordered topics, at most four.
    pub topics: Vec<String>,
    /// Bounded log bytes; never trusted semantic prose.
    pub data: String,
}
/// Local execution evidence. It cannot be converted into approval or signing.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimulationObservation {
    /// SHA-256 commitment to every exact request field; never signing authority.
    pub request_commitment: String,
    /// Exact state/block identity.
    pub block_hash: String,
    /// Pinned engine/rules identity.
    pub engine: String,
    /// Execution status only.
    pub status: ExecutionStatus,
    /// Execution gas used, not total fee qualification.
    pub gas_used: u64,
    /// Pre-refund regular component used for block accounting (EIP-8037/7778).
    pub regular_gas_used: u64,
    /// State component used for block accounting (EIP-8037); zero before activation.
    pub state_gas_used: u64,
    /// Total execution gas spent before refund/floor adjustment.
    pub gas_spent_before_refund: u64,
    /// Effective refund after the transaction gas floor, not extra fee credit.
    pub effective_gas_refund: u64,
    /// Exact transaction calldata floor gas (EIP-7623).
    pub floor_gas: u64,
    /// Bounded return/revert bytes; untrusted.
    pub output: String,
    /// Bounded complete call trace or failure of the entire run.
    pub calls: Vec<CallObservation>,
    /// Untrusted logs from non-reverting execution only.
    pub logs: Vec<LogObservation>,
    /// Changed storage words, not inferred semantic effects.
    pub storage: Vec<StorageChange>,
    /// Changed accounts, including fee changes.
    pub accounts: Vec<AccountChange>,
    /// Always false: raw engine evidence alone never qualifies arbitrary effects.
    pub effects_qualified: bool,
}

// Internally tagged serde envelopes cannot deserialize JSON u128 numbers. Wire
// fees are exact canonical decimal strings, including values above JS/u64 range.
mod u128_string {
    use serde::{Deserialize, Deserializer, Serializer};
    pub(super) fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<u128, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || value.len() > 39
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(serde::de::Error::custom("invalid exact fee"));
        }
        value
            .parse()
            .map_err(|_| serde::de::Error::custom("fee exceeds range"))
    }
}
