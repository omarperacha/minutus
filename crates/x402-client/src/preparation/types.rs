use std::{error::Error, fmt};

use chain_evm::{BaseAssetDescriptor, EvmNetwork, PublicEvmAddress};
use sha2::{Digest as _, Sha256};
use transaction_policy::{ApprovalBindingInput, ApprovalTransactionType};

use crate::{AdmittedPaymentRequirement, ResourceBinding, X402OriginPolicy};

use super::X402_PREPARATION_REVISION;

const CANDIDATE_DIGEST_DOMAIN: &[u8] = b"zeta.x402-prepared-authorization.v1";
const DECODED_DIGEST_DOMAIN: &[u8] = b"zeta.x402-decoded-authorization.v1";

/// Host-supplied public identity of the currently activated wallet selection.
///
/// It contains no wallet identifier, secret, signer, approval or callback.
#[derive(Clone, Eq, PartialEq)]
pub struct ActiveX402WalletAccount {
    address: String,
    network: EvmNetwork,
}

impl ActiveX402WalletAccount {
    /// Validates public facts obtained from the trusted wallet-selection host.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed or zero EVM address.
    pub fn new(address: &str, network: EvmNetwork) -> Result<Self, X402PreparationError> {
        let address = PublicEvmAddress::new(address)
            .map_err(|_| X402PreparationError::InvalidActivatedAccount)?;
        if address.as_str() == "0x0000000000000000000000000000000000000000" {
            return Err(X402PreparationError::InvalidActivatedAccount);
        }
        Ok(Self {
            address: address.as_str().to_owned(),
            network,
        })
    }

    /// Returns the normalized public account.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }
    /// Returns the wallet's selected public network.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
}

/// Closed HTTP methods that can be bound into an x402 preparation request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum X402HttpMethod {
    /// Retrieve the purchased representation.
    Get,
    /// Retrieve only response metadata.
    Head,
    /// Submit a bounded request representation and purchase its response.
    Post,
}

impl X402HttpMethod {
    /// Returns the exact uppercase HTTP spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
        }
    }
}

/// Independently decoded security facts for one EIP-3009 authorization.
///
/// The fields come from the canonical typed-data payload, not the challenge or
/// model explanation. This value contains no signature or signing capability.
#[derive(Clone, Eq, PartialEq)]
pub struct X402AuthorizationFacts {
    domain_name: String,
    domain_version: String,
    chain_id: u64,
    verifying_contract: String,
    account: String,
    recipient: String,
    amount: String,
    valid_after: u64,
    valid_before: u64,
    nonce: [u8; 32],
}

impl X402AuthorizationFacts {
    /// Returns the trusted EIP-712 domain name.
    #[must_use]
    pub fn domain_name(&self) -> &str {
        &self.domain_name
    }
    /// Returns the trusted EIP-712 domain version.
    #[must_use]
    pub fn domain_version(&self) -> &str {
        &self.domain_version
    }
    /// Returns the decoded EIP-155 chain ID.
    #[must_use]
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }
    /// Returns the decoded verifying token contract.
    #[must_use]
    pub fn verifying_contract(&self) -> &str {
        &self.verifying_contract
    }
    /// Returns the decoded payer account.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }
    /// Returns the decoded payment recipient.
    #[must_use]
    pub fn recipient(&self) -> &str {
        &self.recipient
    }
    /// Returns the decoded exact amount in atomic units.
    #[must_use]
    pub fn amount(&self) -> &str {
        &self.amount
    }
    /// Returns the decoded lower validity boundary.
    #[must_use]
    pub const fn valid_after(&self) -> u64 {
        self.valid_after
    }
    /// Returns the decoded exclusive authorization deadline.
    #[must_use]
    pub const fn valid_before(&self) -> u64 {
        self.valid_before
    }
    /// Returns the decoded EIP-3009 replay-protection nonce.
    #[must_use]
    pub const fn nonce(&self) -> &[u8; 32] {
        &self.nonce
    }

    pub(super) fn new(fields: &PreparedFields<'_>) -> Self {
        Self {
            domain_name: crate::policy::usdc_domain_name(fields.chain_id).to_owned(),
            domain_version: "2".to_owned(),
            chain_id: fields.chain_id,
            verifying_contract: fields.asset.to_owned(),
            account: fields.account.to_owned(),
            recipient: fields.recipient.to_owned(),
            amount: fields.amount.to_owned(),
            valid_after: fields.valid_after,
            valid_before: fields.valid_before,
            nonce: fields.nonce,
        }
    }

    fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(DECODED_DIGEST_DOMAIN);
        hash_field(&mut hasher, self.domain_name.as_bytes());
        hash_field(&mut hasher, self.domain_version.as_bytes());
        hash_field(&mut hasher, &self.chain_id.to_be_bytes());
        hash_field(&mut hasher, self.verifying_contract.as_bytes());
        hash_field(&mut hasher, self.account.as_bytes());
        hash_field(&mut hasher, self.recipient.as_bytes());
        hash_field(&mut hasher, self.amount.as_bytes());
        hash_field(&mut hasher, &self.valid_after.to_be_bytes());
        hash_field(&mut hasher, &self.valid_before.to_be_bytes());
        hash_field(&mut hasher, &self.nonce);
        hasher.finalize().into()
    }
}

/// Immutable digest-addressed authorization awaiting trusted approval.
///
/// This type deliberately implements neither `Debug` nor serialization because
/// the exact resource URL can contain a sensitive query. It has no wallet,
/// signing, HTTP, facilitator or retry capability.
#[derive(Clone, PartialEq)]
pub struct PreparedX402Authorization {
    preparation_revision: u32,
    policy_revision: u32,
    origin_policy: X402OriginPolicy,
    requirement_index: usize,
    max_timeout_seconds: u64,
    resource: ResourceBinding,
    payment_extensions: Option<serde_json::Map<String, serde_json::Value>>,
    method: X402HttpMethod,
    request_identity_sha256: [u8; 32],
    network: EvmNetwork,
    asset: BaseAssetDescriptor,
    decoded: X402AuthorizationFacts,
    canonical_typed_data: String,
    signing_digest: [u8; 32],
    candidate_digest: [u8; 32],
}

impl PreparedX402Authorization {
    pub(crate) fn payment_extensions(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.payment_extensions.as_ref()
    }

    pub(super) fn new(
        requirement: &AdmittedPaymentRequirement,
        method: X402HttpMethod,
        request_identity_sha256: [u8; 32],
        canonical_typed_data: String,
        signing_digest: [u8; 32],
        decoded: X402AuthorizationFacts,
    ) -> Self {
        let candidate_digest = candidate_digest(
            requirement,
            method,
            request_identity_sha256,
            &canonical_typed_data,
            signing_digest,
            decoded.digest(),
        );
        Self {
            preparation_revision: X402_PREPARATION_REVISION,
            policy_revision: requirement.policy_revision(),
            origin_policy: requirement.origin_policy(),
            requirement_index: requirement.requirement_index(),
            max_timeout_seconds: requirement.max_timeout_seconds(),
            resource: requirement.resource().clone(),
            payment_extensions: requirement.payment_extensions().cloned(),
            method,
            request_identity_sha256,
            network: requirement.network(),
            asset: requirement.asset(),
            decoded,
            canonical_typed_data,
            signing_digest,
            candidate_digest,
        }
    }

    /// Returns the preparation contract revision.
    #[must_use]
    pub const fn preparation_revision(&self) -> u32 {
        self.preparation_revision
    }
    /// Returns the policy revision used during admission.
    #[must_use]
    pub const fn policy_revision(&self) -> u32 {
        self.policy_revision
    }
    /// Returns the retained origin policy used at admission.
    #[must_use]
    pub const fn origin_policy(&self) -> X402OriginPolicy {
        self.origin_policy
    }
    /// Returns the exact selected challenge requirement index.
    #[must_use]
    pub const fn requirement_index(&self) -> usize {
        self.requirement_index
    }
    /// Returns the admitted completion window echoed in the payment payload.
    #[must_use]
    pub const fn max_timeout_seconds(&self) -> u64 {
        self.max_timeout_seconds
    }
    /// Returns the original canonical protected resource.
    #[must_use]
    pub const fn resource(&self) -> &ResourceBinding {
        &self.resource
    }
    /// Returns the exact bound HTTP method.
    #[must_use]
    pub const fn method(&self) -> X402HttpMethod {
        self.method
    }
    /// Returns the host-owned exact original-request identity.
    #[must_use]
    pub const fn request_identity_sha256(&self) -> &[u8; 32] {
        &self.request_identity_sha256
    }
    /// Returns the admitted EVM network.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    /// Returns the admitted chain-bound asset descriptor.
    #[must_use]
    pub const fn asset(&self) -> BaseAssetDescriptor {
        self.asset
    }
    /// Returns facts decoded independently from the typed-data payload.
    #[must_use]
    pub const fn decoded(&self) -> &X402AuthorizationFacts {
        &self.decoded
    }
    /// Returns the deterministic canonical EIP-712 JSON to be reviewed.
    #[must_use]
    pub fn canonical_typed_data(&self) -> &str {
        &self.canonical_typed_data
    }
    /// Returns the exact EIP-712 digest that a later approval may authorize.
    #[must_use]
    pub const fn signing_digest(&self) -> &[u8; 32] {
        &self.signing_digest
    }
    /// Returns the digest of the complete request-bound candidate.
    #[must_use]
    pub const fn candidate_digest(&self) -> &[u8; 32] {
        &self.candidate_digest
    }
    /// Returns whether the underlying authorization has expired.
    #[must_use]
    pub const fn is_expired_at(&self, now_unix_seconds: u64) -> bool {
        now_unix_seconds >= self.decoded.valid_before
    }

    /// Builds non-authoritative input for Epic 13's later native approval step.
    ///
    /// The candidate digest occupies the decoder commitment so the approval is
    /// bound to both independently decoded EIP-712 facts and the original HTTP
    /// request context. This returned value is public data, not an approval or
    /// signing capability.
    ///
    /// # Errors
    ///
    /// Returns an error if the proposed approval outlives this authorization.
    pub fn approval_binding_input<'a>(
        &'a self,
        session_id: &'a str,
        approval_expires_at_unix: u64,
        simulation_digest: [u8; 32],
    ) -> Result<ApprovalBindingInput<'a>, X402PreparationError> {
        if approval_expires_at_unix > self.decoded.valid_before {
            return Err(X402PreparationError::ApprovalOutlivesAuthorization);
        }
        Ok(ApprovalBindingInput {
            signing_digest: self.signing_digest,
            chain_id: self.decoded.chain_id,
            account: &self.decoded.account,
            session_id,
            expires_at_unix: approval_expires_at_unix,
            transaction_type: ApprovalTransactionType::Eip712,
            decoder_digest: self.candidate_digest,
            simulation_digest,
        })
    }
}

pub(super) struct PreparedFields<'a> {
    pub(super) account: &'a str,
    pub(super) recipient: &'a str,
    pub(super) amount: &'a str,
    pub(super) valid_after: u64,
    pub(super) valid_before: u64,
    pub(super) nonce: [u8; 32],
    pub(super) chain_id: u64,
    pub(super) asset: &'a str,
}

/// Closed preparation failure suitable for trusted warnings and audit codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum X402PreparationError {
    /// The admitted evidence was produced by another policy revision.
    StalePolicy,
    /// The challenge deadline has passed.
    ExpiredRequirement,
    /// The original request did not carry a usable host-owned identity.
    InvalidRequestIdentity,
    /// Too little challenge lifetime remains for meaningful human approval.
    ExpiryTooNear,
    /// The activated public wallet account is malformed or the zero address.
    InvalidActivatedAccount,
    /// No unlocked and activated public wallet selection was supplied.
    WalletUnavailable,
    /// The activated wallet was selected for another EVM network.
    WalletNetworkMismatch,
    /// The admitted asset did not contain a token contract.
    UnsupportedAsset,
    /// Secure EIP-3009 nonce generation was unavailable.
    RandomnessUnavailable,
    /// A supplied nonce was invalid.
    InvalidNonce,
    /// Canonical typed-data construction failed.
    EncodingFailed,
    /// The complete payment payload would exceed the header limit before signing.
    PaymentHeaderLimit,
    /// Independent typed-data decoding or comparison failed.
    CandidateMismatch,
    /// A proposed native approval would outlive the authorization.
    ApprovalOutlivesAuthorization,
}

impl X402PreparationError {
    /// Returns a stable bounded failure code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StalePolicy => "stale_policy",
            Self::ExpiredRequirement => "expired_requirement",
            Self::InvalidRequestIdentity => "invalid_request_identity",
            Self::ExpiryTooNear => "expiry_too_near",
            Self::InvalidActivatedAccount => "invalid_activated_account",
            Self::WalletUnavailable => "wallet_unavailable",
            Self::WalletNetworkMismatch => "wallet_network_mismatch",
            Self::UnsupportedAsset => "unsupported_asset",
            Self::RandomnessUnavailable => "randomness_unavailable",
            Self::InvalidNonce => "invalid_nonce",
            Self::EncodingFailed => "encoding_failed",
            Self::PaymentHeaderLimit => "payment_header_limit",
            Self::CandidateMismatch => "candidate_mismatch",
            Self::ApprovalOutlivesAuthorization => "approval_outlives_authorization",
        }
    }
}

impl fmt::Display for X402PreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PaymentHeaderLimit => "x402 payment payload exceeds the header limit",
            Self::StalePolicy => "x402 policy evidence is stale",
            Self::ExpiredRequirement => "x402 payment requirement has expired",
            Self::InvalidRequestIdentity => "original x402 request identity is invalid",
            Self::ExpiryTooNear => "x402 payment requirement expires too soon for approval",
            Self::InvalidActivatedAccount => "activated x402 wallet account is invalid",
            Self::WalletUnavailable => "activated x402 wallet is unavailable",
            Self::WalletNetworkMismatch => "activated x402 wallet network does not match",
            Self::UnsupportedAsset => "x402 payment asset cannot authorize a token transfer",
            Self::RandomnessUnavailable => "secure x402 authorization nonce is unavailable",
            Self::InvalidNonce => "x402 authorization nonce is invalid",
            Self::EncodingFailed => "x402 typed authorization could not be encoded",
            Self::CandidateMismatch => "decoded x402 authorization differs from admitted facts",
            Self::ApprovalOutlivesAuthorization => {
                "x402 approval expiry exceeds the authorization deadline"
            }
        })
    }
}

impl Error for X402PreparationError {}

fn candidate_digest(
    requirement: &AdmittedPaymentRequirement,
    method: X402HttpMethod,
    request_identity_sha256: [u8; 32],
    typed_data: &str,
    signing_digest: [u8; 32],
    decoded_digest: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CANDIDATE_DIGEST_DOMAIN);
    hash_field(&mut hasher, &X402_PREPARATION_REVISION.to_be_bytes());
    hash_field(&mut hasher, &requirement.policy_revision().to_be_bytes());
    hash_field(&mut hasher, requirement.origin_policy().as_str().as_bytes());
    hash_field(
        &mut hasher,
        requirement.requirement_index().to_string().as_bytes(),
    );
    hash_field(
        &mut hasher,
        &requirement.max_timeout_seconds().to_be_bytes(),
    );
    hash_field(&mut hasher, method.as_str().as_bytes());
    hash_field(
        &mut hasher,
        requirement.resource().canonical_url().as_bytes(),
    );
    hash_field(&mut hasher, requirement.resource().origin().as_bytes());
    // Bind the exact echoed metadata into approval without giving it any
    // influence over the independently decoded EIP-3009 payment facts.
    let extensions = requirement
        .payment_extensions()
        .map_or_else(String::new, |value| {
            serde_json::Value::Object(value.clone()).to_string()
        });
    hash_field(&mut hasher, extensions.as_bytes());
    hash_field(&mut hasher, &request_identity_sha256);
    hash_field(&mut hasher, typed_data.as_bytes());
    hash_field(&mut hasher, &signing_digest);
    hash_field(&mut hasher, &decoded_digest);
    hasher.finalize().into()
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}
