use std::{error::Error, fmt};

use transaction_policy::ApprovalBindingInput;

use crate::{PreparedX402Authorization, X402PreparationError};

use super::MAX_X402_AGENT_REASON_BYTES;

/// Bounded model-authored context, always rendered as untrusted.
#[derive(Clone, Eq, PartialEq)]
pub struct X402AgentReason(String);

impl X402AgentReason {
    pub(super) fn new(value: &str) -> Result<Self, X402ApprovalError> {
        if value.is_empty()
            || value.len() > MAX_X402_AGENT_REASON_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(X402ApprovalError::InvalidAgentReason);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the bounded, untrusted reason text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Closed warnings derived only from trusted candidate and host facts.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum X402ApprovalWarning {
    /// Payment targets the exact debug-only local emulator route.
    LocalDevelopmentEndpoint,
    /// Payment is on a public test network.
    Testnet,
    /// Recipient is server-selected and not present in trusted configuration.
    UnrecognizedRecipient,
    /// The authorization or approval is close to expiry.
    ExpiringAuthorization,
    /// Exact price exceeds the explicit development review threshold.
    UnusualPrice,
}

impl X402ApprovalWarning {
    /// Stable bounded warning code for trusted UI and audit mapping.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::LocalDevelopmentEndpoint => "local_development_endpoint",
            Self::Testnet => "testnet",
            Self::UnrecognizedRecipient => "unrecognized_recipient",
            Self::ExpiringAuthorization => "expiring_authorization",
            Self::UnusualPrice => "unusual_price",
        }
    }

    /// Prominent trusted warning text.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::LocalDevelopmentEndpoint => {
                "LOCAL DEVELOPMENT ENDPOINT: this payment targets a configured emulator on this Mac."
            }
            Self::Testnet => "TESTNET: Base Sepolia assets may have no financial value.",
            Self::UnrecognizedRecipient => {
                "UNRECOGNIZED RECIPIENT: the server selected this pay-to address; verify it carefully."
            }
            Self::ExpiringAuthorization => {
                "EXPIRING SOON: this exact payment authorization has little time remaining."
            }
            Self::UnusualPrice => {
                "UNUSUAL PRICE: the requested amount exceeds the development review threshold."
            }
        }
    }
}

/// Trusted, native-ready x402 payment facts.
///
/// This value deliberately implements neither `Debug` nor serialization because
/// the exact resource URL can contain sensitive query values.
#[derive(Clone, Eq, PartialEq)]
pub struct X402ApprovalPresentation {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) domain: String,
    pub(super) resource: String,
    pub(super) price: String,
    pub(super) atomic_amount: String,
    pub(super) asset_symbol: String,
    pub(super) asset_contract: String,
    pub(super) network: String,
    pub(super) payer: String,
    pub(super) recipient: String,
    pub(super) expires_at_unix: u64,
    pub(super) agent_reason: Option<X402AgentReason>,
    pub(super) warnings: Vec<X402ApprovalWarning>,
}

impl X402ApprovalPresentation {
    /// Native alert title, including a warning count when applicable.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }
    /// Fully labeled native body with trusted facts before untrusted context.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }
    /// Canonical protected HTTPS origin.
    #[must_use]
    pub fn domain(&self) -> &str {
        &self.domain
    }
    /// Canonical exact resource URL.
    #[must_use]
    pub fn resource(&self) -> &str {
        &self.resource
    }
    /// Exact decimal token price with trusted symbol.
    #[must_use]
    pub fn price(&self) -> &str {
        &self.price
    }
    /// Exact atomic-unit amount.
    #[must_use]
    pub fn atomic_amount(&self) -> &str {
        &self.atomic_amount
    }
    /// Trusted asset symbol.
    #[must_use]
    pub fn asset_symbol(&self) -> &str {
        &self.asset_symbol
    }
    /// Trusted chain-bound token contract.
    #[must_use]
    pub fn asset_contract(&self) -> &str {
        &self.asset_contract
    }
    /// Trusted network display including CAIP-2 and chain ID.
    #[must_use]
    pub fn network(&self) -> &str {
        &self.network
    }
    /// Independently decoded paying account.
    #[must_use]
    pub fn payer(&self) -> &str {
        &self.payer
    }
    /// Independently decoded pay-to address.
    #[must_use]
    pub fn recipient(&self) -> &str {
        &self.recipient
    }
    /// Exact approval expiry, never later than the authorization.
    #[must_use]
    pub const fn expires_at_unix(&self) -> u64 {
        self.expires_at_unix
    }
    /// Explicitly untrusted agent context, if supplied.
    #[must_use]
    pub const fn agent_reason(&self) -> Option<&X402AgentReason> {
        self.agent_reason.as_ref()
    }
    /// Trusted warnings in stable display order.
    #[must_use]
    pub fn warnings(&self) -> &[X402ApprovalWarning] {
        &self.warnings
    }
}

/// Prepared native review evidence for one exact x402 candidate.
///
/// It is process-local, non-serializable and conveys no approval or signing
/// authority. The native host retains it until a distinct decision is made.
#[derive(Clone, PartialEq)]
pub struct PreparedX402Approval {
    candidate: PreparedX402Authorization,
    wallet_session_id: String,
    expires_at_unix: u64,
    validation_digest: [u8; 32],
    presentation: X402ApprovalPresentation,
}

impl PreparedX402Approval {
    pub(super) fn new(
        candidate: PreparedX402Authorization,
        wallet_session_id: &str,
        expires_at_unix: u64,
        validation_digest: [u8; 32],
        presentation: X402ApprovalPresentation,
    ) -> Self {
        Self {
            candidate,
            wallet_session_id: wallet_session_id.to_owned(),
            expires_at_unix,
            validation_digest,
            presentation,
        }
    }

    /// Exact immutable payment candidate retained by the trusted host.
    #[must_use]
    pub const fn candidate(&self) -> &PreparedX402Authorization {
        &self.candidate
    }
    /// Trusted native presentation derived from that candidate.
    #[must_use]
    pub const fn presentation(&self) -> &X402ApprovalPresentation {
        &self.presentation
    }
    /// Host-created wallet session bound into any later commitment.
    #[must_use]
    pub fn wallet_session_id(&self) -> &str {
        &self.wallet_session_id
    }
    /// Exact approval deadline.
    #[must_use]
    pub const fn expires_at_unix(&self) -> u64 {
        self.expires_at_unix
    }
    /// Whether the review has reached its terminal expiry.
    #[must_use]
    pub const fn is_expired_at(&self, now_unix: u64) -> bool {
        now_unix >= self.expires_at_unix
    }

    /// Returns public Epic 13 binding evidence for native host issuance.
    ///
    /// This does not issue an approval commitment. The host must call it only
    /// as part of handling an explicit native `Approved` decision.
    ///
    /// # Errors
    ///
    /// Returns a closed error if the retained approval would outlive the
    /// authorization, which indicates invalid trusted state.
    pub fn binding_input(&self) -> Result<ApprovalBindingInput<'_>, X402ApprovalError> {
        self.candidate
            .approval_binding_input(
                &self.wallet_session_id,
                self.expires_at_unix,
                self.validation_digest,
            )
            .map_err(X402ApprovalError::from_preparation)
    }
}

/// Distinct terminal result returned by the trusted native decision surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum X402ApprovalDecision {
    /// User selected the native approval control for the exact payment.
    Approved,
    /// User explicitly denied the payment.
    Denied,
    /// Native review closed without an approval.
    Dismissed,
    /// Trusted host time reached the bound expiry before approval.
    Expired,
}

/// Closed preparation or presentation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum X402ApprovalError {
    /// Candidate policy evidence is not current.
    StalePolicy,
    /// Candidate or review has expired.
    Expired,
    /// Host wallet session identifier is malformed.
    InvalidWalletSession,
    /// Agent context is empty, excessive or contains presentation controls.
    InvalidAgentReason,
    /// Exact typed authorization could not be independently revalidated.
    ValidationUnavailable,
    /// Trusted asset denomination could not be represented safely.
    InvalidAssetAmount,
    /// Approval evidence would outlive its authorization.
    ApprovalOutlivesAuthorization,
}

impl X402ApprovalError {
    pub(super) const fn from_preparation(error: X402PreparationError) -> Self {
        match error {
            X402PreparationError::ApprovalOutlivesAuthorization => {
                Self::ApprovalOutlivesAuthorization
            }
            _ => Self::ValidationUnavailable,
        }
    }

    /// Stable bounded error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StalePolicy => "stale_policy",
            Self::Expired => "expired",
            Self::InvalidWalletSession => "invalid_wallet_session",
            Self::InvalidAgentReason => "invalid_agent_reason",
            Self::ValidationUnavailable => "validation_unavailable",
            Self::InvalidAssetAmount => "invalid_asset_amount",
            Self::ApprovalOutlivesAuthorization => "approval_outlives_authorization",
        }
    }
}

impl fmt::Display for X402ApprovalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StalePolicy => "x402 approval policy evidence is stale",
            Self::Expired => "x402 approval has expired",
            Self::InvalidWalletSession => "x402 wallet session is invalid",
            Self::InvalidAgentReason => "x402 agent reason is unsafe to display",
            Self::ValidationUnavailable => "x402 authorization validation is unavailable",
            Self::InvalidAssetAmount => "x402 price cannot be rendered exactly",
            Self::ApprovalOutlivesAuthorization => "x402 approval outlives its authorization",
        })
    }
}

impl Error for X402ApprovalError {}
