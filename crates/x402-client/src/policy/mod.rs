//! Closed public HTTPS and debug qualification payment policies.

mod extensions;

use std::{error::Error, fmt};

use chain_evm::{BaseAssetDescriptor, EvmNetwork, trusted_assets};
use serde_json::{Map, Value};
use url::{Host, Url};

use crate::{PaymentRequiredChallenge, ResourceBinding};

/// Revision of the x402 payment policy bound into admitted requirements.
pub const X402_POLICY_REVISION: u32 = 4;

/// Closed origin policy retained with an admitted payment requirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum X402OriginPolicy {
    /// Canonical public HTTPS origin admitted by the production policy.
    PublicHttps,
    /// Configured loopback development resource, available only in debug builds.
    #[cfg(debug_assertions)]
    DebugLoopback,
}

impl X402OriginPolicy {
    /// Stable identifier bound into prepared authorization evidence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PublicHttps => "public_https",
            #[cfg(debug_assertions)]
            Self::DebugLoopback => "debug_loopback",
        }
    }
}

const SUPPORTED_SCHEME: &str = "exact";
const SUPPORTED_ASSET_TRANSFER_METHOD: &str = "eip3009";
const SUPPORTED_PAYMENT_FLOW: &str = "authorization";
pub(crate) const fn usdc_domain_name(chain_id: u64) -> &'static str {
    if chain_id == 8453 { "USD Coin" } else { "USDC" }
}
const USDC_DOMAIN_VERSION: &str = "2";

/// Closed payment scheme admitted by the x402 policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupportedX402Scheme {
    /// Exact EVM payment using USDC `transferWithAuthorization` (EIP-3009).
    ExactEip3009,
}

impl SupportedX402Scheme {
    /// Returns the x402 scheme identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactEip3009 => SUPPORTED_SCHEME,
        }
    }

    /// Returns the selected EVM asset transfer method.
    #[must_use]
    pub const fn asset_transfer_method(self) -> &'static str {
        match self {
            Self::ExactEip3009 => SUPPORTED_ASSET_TRANSFER_METHOD,
        }
    }

    /// Returns the supported x402 payment flow.
    #[must_use]
    pub const fn payment_flow(self) -> &'static str {
        match self {
            Self::ExactEip3009 => SUPPORTED_PAYMENT_FLOW,
        }
    }
}

/// One parsed requirement admitted by the exact payment policy.
///
/// The value is immutable and grants no signing, wallet, network or retry
/// capability. Retained resource URLs and untrusted discovery metadata may
/// contain sensitive data, so it deliberately has no `Debug` implementation.
#[derive(Clone, PartialEq)]
pub struct AdmittedPaymentRequirement {
    policy_revision: u32,
    origin_policy: X402OriginPolicy,
    requirement_index: usize,
    resource: ResourceBinding,
    payment_extensions: Option<Map<String, Value>>,
    scheme: SupportedX402Scheme,
    network: EvmNetwork,
    asset: BaseAssetDescriptor,
    amount: String,
    recipient: String,
    max_timeout_seconds: u64,
    expires_at_unix_seconds: u64,
}

impl AdmittedPaymentRequirement {
    pub(crate) fn payment_extensions(&self) -> Option<&Map<String, Value>> {
        self.payment_extensions.as_ref()
    }

    /// Returns the policy revision that admitted this requirement.
    #[must_use]
    pub const fn policy_revision(&self) -> u32 {
        self.policy_revision
    }

    /// Returns the exact origin policy that admitted this requirement.
    #[must_use]
    pub const fn origin_policy(&self) -> X402OriginPolicy {
        self.origin_policy
    }

    /// Returns the requirement's position in the parsed `accepts` array.
    #[must_use]
    pub const fn requirement_index(&self) -> usize {
        self.requirement_index
    }

    /// Returns the trusted original resource binding.
    #[must_use]
    pub const fn resource(&self) -> &ResourceBinding {
        &self.resource
    }

    /// Returns the closed supported payment scheme.
    #[must_use]
    pub const fn scheme(&self) -> SupportedX402Scheme {
        self.scheme
    }

    /// Returns the trusted payment network.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }

    /// Returns the trusted chain-bound USDC descriptor.
    #[must_use]
    pub const fn asset(&self) -> BaseAssetDescriptor {
        self.asset
    }

    /// Returns the exact positive price in atomic USDC units.
    #[must_use]
    pub fn amount(&self) -> &str {
        &self.amount
    }

    /// Returns the normalized lowercase public payment recipient.
    #[must_use]
    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    /// Returns the admitted maximum completion window.
    #[must_use]
    pub const fn max_timeout_seconds(&self) -> u64 {
        self.max_timeout_seconds
    }

    /// Returns the local deadline derived from trusted response-receipt time.
    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    /// Returns whether the admitted requirement has reached its deadline.
    #[must_use]
    pub const fn is_expired_at(&self, now_unix_seconds: u64) -> bool {
        now_unix_seconds >= self.expires_at_unix_seconds
    }
}

/// Typed x402 development-policy rejection suitable for bounded audit codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum X402PolicyError {
    /// The selected requirement index was absent from the bounded challenge.
    UnknownRequirement,
    /// The resource did not use HTTPS.
    InsecureOrigin,
    /// The resource used a non-default HTTPS port.
    DisallowedPort,
    /// The resource host was not a canonical multi-label DNS domain.
    DisallowedDomain,
    /// The challenge advertised an unsupported protocol extension.
    UnsupportedExtension,
    /// A supported extension did not provide its required metadata envelope.
    InvalidExtensionData,
    /// The selected payment scheme was not explicitly supported.
    UnsupportedScheme,
    /// The selected CAIP-2 network is not supported by this policy.
    UnsupportedNetwork,
    /// The selected asset was not the trusted USDC contract for this network.
    UnsupportedAsset,
    /// Scheme-specific data was absent, malformed or contained unknown fields.
    InvalidSchemeData,
    /// The selected asset transfer method was not EIP-3009.
    UnsupportedAssetTransferMethod,
    /// The selected payment flow was not the authorization flow.
    UnsupportedPaymentFlow,
    /// The requirement's local deadline has passed.
    ExpiredRequirement,
}

impl fmt::Display for X402PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnknownRequirement => "x402 payment requirement does not exist",
            Self::InsecureOrigin => "x402 payment requires an HTTPS resource",
            Self::DisallowedPort => "x402 payment resource uses a disallowed port",
            Self::DisallowedDomain => "x402 payment resource domain is not allowed",
            Self::UnsupportedExtension => "x402 protocol extension is not supported",
            Self::InvalidExtensionData => "x402 extension metadata is invalid",
            Self::UnsupportedScheme => "x402 payment scheme is not supported",
            Self::UnsupportedNetwork => "x402 payment network is not supported",
            Self::UnsupportedAsset => "x402 payment asset is not supported",
            Self::InvalidSchemeData => "x402 scheme data is invalid",
            Self::UnsupportedAssetTransferMethod => "x402 asset transfer method is not supported",
            Self::UnsupportedPaymentFlow => "x402 payment flow is not supported",
            Self::ExpiredRequirement => "x402 payment requirement has expired",
        })
    }
}

impl Error for X402PolicyError {}

impl X402PolicyError {
    /// Returns the stable bounded reason code for audit and warning mapping.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownRequirement => "unknown_requirement",
            Self::InsecureOrigin => "insecure_origin",
            Self::DisallowedPort => "disallowed_port",
            Self::DisallowedDomain => "disallowed_domain",
            Self::UnsupportedExtension => "unsupported_extension",
            Self::InvalidExtensionData => "invalid_extension_data",
            Self::UnsupportedScheme => "unsupported_scheme",
            Self::UnsupportedNetwork => "unsupported_network",
            Self::UnsupportedAsset => "unsupported_asset",
            Self::InvalidSchemeData => "invalid_scheme_data",
            Self::UnsupportedAssetTransferMethod => "unsupported_asset_transfer_method",
            Self::UnsupportedPaymentFlow => "unsupported_payment_flow",
            Self::ExpiredRequirement => "expired_requirement",
        }
    }
}

/// Applies the complete public HTTPS allow-list to one parsed requirement.
///
/// Call this before preparation and repeat it with fresh trusted host time
/// before signing and retry. A successful result is public immutable evidence,
/// not payment authority.
///
/// # Errors
///
/// Returns [`X402PolicyError`] unless the resource is a canonical HTTPS domain
/// on its default port and the requirement is exact EIP-3009 USDC on Base
/// mainnet or Base Sepolia with no unsupported extensions or payment modes.
pub fn admit_payment_requirement(
    challenge: &PaymentRequiredChallenge,
    requirement_index: usize,
    now_unix_seconds: u64,
) -> Result<AdmittedPaymentRequirement, X402PolicyError> {
    validate_origin(challenge.resource())?;
    validate_x402_payment_terms(challenge, requirement_index, now_unix_seconds)?;
    let requirement = &challenge.requirements()[requirement_index];
    let network = supported_network(requirement.network())?;
    Ok(AdmittedPaymentRequirement {
        policy_revision: X402_POLICY_REVISION,
        origin_policy: X402OriginPolicy::PublicHttps,
        requirement_index,
        resource: challenge.resource().clone(),
        payment_extensions: challenge
            .untrusted_extensions()
            .filter(|value| !value.is_empty())
            .cloned(),
        scheme: SupportedX402Scheme::ExactEip3009,
        network,
        asset: trusted_usdc(network),
        amount: requirement.amount().to_owned(),
        recipient: requirement.recipient().to_owned(),
        max_timeout_seconds: requirement.max_timeout_seconds(),
        expires_at_unix_seconds: requirement.expires_at_unix_seconds(),
    })
}

/// Admits a resource on the caller's configured debug loopback origin.
///
/// This entry point does not exist in release builds and cannot relax the
/// production HTTPS policy.
///
/// # Errors
///
/// Returns a closed policy error unless both the exact local route and all
/// supported Base Sepolia payment terms validate.
#[cfg(debug_assertions)]
pub fn admit_debug_loopback_payment_requirement(
    challenge: &PaymentRequiredChallenge,
    requirement_index: usize,
    now_unix_seconds: u64,
    expected_origin: &str,
) -> Result<AdmittedPaymentRequirement, X402PolicyError> {
    validate_debug_loopback_origin(challenge.resource(), Some(expected_origin))?;
    validate_x402_payment_terms(challenge, requirement_index, now_unix_seconds)?;
    let requirement = &challenge.requirements()[requirement_index];
    if requirement.network() != "eip155:84532" {
        return Err(X402PolicyError::UnsupportedNetwork);
    }
    let network = EvmNetwork::BaseSepolia;
    Ok(AdmittedPaymentRequirement {
        policy_revision: X402_POLICY_REVISION,
        origin_policy: X402OriginPolicy::DebugLoopback,
        requirement_index,
        resource: challenge.resource().clone(),
        payment_extensions: challenge
            .untrusted_extensions()
            .filter(|value| !value.is_empty())
            .cloned(),
        scheme: SupportedX402Scheme::ExactEip3009,
        network,
        asset: trusted_usdc(network),
        amount: requirement.amount().to_owned(),
        recipient: requirement.recipient().to_owned(),
        max_timeout_seconds: requirement.max_timeout_seconds(),
        expires_at_unix_seconds: requirement.expires_at_unix_seconds(),
    })
}

/// Inspects supported payment terms without admitting a resource or payment.
///
/// Used by the unpaid emulator preflight. This deliberately does not check
/// origin policy or return an [`AdmittedPaymentRequirement`]; success cannot
/// be used to prepare an authorization. Payment callers must continue to use
/// [`admit_payment_requirement`], which also enforces the origin boundary.
///
/// # Errors
///
/// Rejects unsupported extensions, scheme, network, asset, scheme data or
/// expired terms using the same checks as full payment admission.
pub fn validate_x402_payment_terms(
    challenge: &PaymentRequiredChallenge,
    requirement_index: usize,
    now_unix_seconds: u64,
) -> Result<(), X402PolicyError> {
    extensions::validate_payment_extensions(challenge.untrusted_extensions())?;
    let requirement = challenge
        .requirements()
        .get(requirement_index)
        .ok_or(X402PolicyError::UnknownRequirement)?;
    if requirement.scheme() != SUPPORTED_SCHEME {
        return Err(X402PolicyError::UnsupportedScheme);
    }

    let network = supported_network(requirement.network())?;
    let asset = trusted_usdc(network);
    if asset.token_contract() != Some(requirement.asset()) {
        return Err(X402PolicyError::UnsupportedAsset);
    }
    validate_scheme_data(requirement.untrusted_extra(), network)?;
    if requirement.is_expired_at(now_unix_seconds) {
        return Err(X402PolicyError::ExpiredRequirement);
    }

    Ok(())
}

fn validate_origin(resource: &ResourceBinding) -> Result<(), X402PolicyError> {
    let url =
        Url::parse(resource.canonical_url()).map_err(|_| X402PolicyError::DisallowedDomain)?;
    if url.scheme() != "https" {
        return Err(X402PolicyError::InsecureOrigin);
    }
    if url.port_or_known_default() != Some(443) || url.port().is_some() {
        return Err(X402PolicyError::DisallowedPort);
    }
    let Host::Domain(domain) = url.host().ok_or(X402PolicyError::DisallowedDomain)? else {
        return Err(X402PolicyError::DisallowedDomain);
    };
    if is_disallowed_domain(domain) {
        return Err(X402PolicyError::DisallowedDomain);
    }
    Ok(())
}

pub(crate) fn revalidate_prepared_policy(
    resource: &ResourceBinding,
    origin_policy: X402OriginPolicy,
    network: EvmNetwork,
    asset: BaseAssetDescriptor,
    payment_extensions: Option<&Map<String, Value>>,
    expires_at_unix_seconds: u64,
    now_unix_seconds: u64,
) -> Result<(), X402PolicyError> {
    extensions::validate_payment_extensions(payment_extensions)?;
    match origin_policy {
        X402OriginPolicy::PublicHttps => validate_origin(resource)?,
        #[cfg(debug_assertions)]
        X402OriginPolicy::DebugLoopback => validate_debug_loopback_origin(resource, None)?,
    }
    if !matches!(network, EvmNetwork::BaseSepolia | EvmNetwork::BaseMainnet)
        || (origin_policy.as_str() == "debug_loopback" && network != EvmNetwork::BaseSepolia)
    {
        return Err(X402PolicyError::UnsupportedNetwork);
    }
    if asset != trusted_usdc(network) {
        return Err(X402PolicyError::UnsupportedAsset);
    }
    if now_unix_seconds >= expires_at_unix_seconds {
        return Err(X402PolicyError::ExpiredRequirement);
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn validate_debug_loopback_origin(
    resource: &ResourceBinding,
    expected_origin: Option<&str>,
) -> Result<(), X402PolicyError> {
    let url =
        Url::parse(resource.canonical_url()).map_err(|_| X402PolicyError::DisallowedDomain)?;
    if url.scheme() != "http"
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_none()
        || !matches!(url.host(), Some(Host::Ipv4(address)) if address.is_loopback())
        || expected_origin.is_some_and(|origin| resource.origin() != origin)
    {
        return Err(X402PolicyError::DisallowedDomain);
    }
    Ok(())
}

fn is_disallowed_domain(domain: &str) -> bool {
    domain.ends_with('.')
        || !domain.contains('.')
        || domain.split('.').any(str::is_empty)
        || domain == "localhost"
        || [
            ".localhost",
            ".local",
            ".internal",
            ".test",
            ".invalid",
            ".example",
            ".onion",
            ".arpa",
            ".alt",
        ]
        .iter()
        .any(|suffix| domain.ends_with(suffix))
}

fn supported_network(caip2: &str) -> Result<EvmNetwork, X402PolicyError> {
    match caip2 {
        "eip155:8453" => Ok(EvmNetwork::BaseMainnet),
        "eip155:84532" => Ok(EvmNetwork::BaseSepolia),
        _ => Err(X402PolicyError::UnsupportedNetwork),
    }
}

fn trusted_usdc(network: EvmNetwork) -> BaseAssetDescriptor {
    let [_, usdc] = trusted_assets(network);
    usdc
}

fn validate_scheme_data(
    extra: Option<&Map<String, Value>>,
    network: EvmNetwork,
) -> Result<(), X402PolicyError> {
    let extra = extra.ok_or(X402PolicyError::InvalidSchemeData)?;
    if extra.keys().any(|key| {
        !matches!(
            key.as_str(),
            "assetTransferMethod" | "name" | "paymentFlow" | "version"
        )
    }) {
        return Err(X402PolicyError::InvalidSchemeData);
    }
    if extra.get("name").and_then(Value::as_str)
        != Some(usdc_domain_name(network.descriptor().chain_id()))
        || extra.get("version").and_then(Value::as_str) != Some(USDC_DOMAIN_VERSION)
    {
        return Err(X402PolicyError::InvalidSchemeData);
    }
    if extra
        .get("assetTransferMethod")
        .is_some_and(|method| method.as_str() != Some(SUPPORTED_ASSET_TRANSFER_METHOD))
    {
        return Err(X402PolicyError::UnsupportedAssetTransferMethod);
    }
    if extra
        .get("paymentFlow")
        .is_some_and(|flow| flow.as_str() != Some(SUPPORTED_PAYMENT_FLOW))
    {
        return Err(X402PolicyError::UnsupportedPaymentFlow);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
