use std::{error::Error, fmt};

use serde_json::{Map, Value};

/// One validated x402 v2 payment-required challenge.
///
/// This type deliberately omits remote descriptions and error prose. The
/// resource identity comes from the trusted original request context.
#[derive(Clone, PartialEq)]
pub struct PaymentRequiredChallenge {
    pub(super) resource: ResourceBinding,
    pub(super) requirements: Vec<PaymentRequirement>,
    pub(super) untrusted_extensions: Option<Map<String, Value>>,
}

impl PaymentRequiredChallenge {
    /// Returns the protocol version accepted by this parser.
    #[must_use]
    pub const fn version(&self) -> u8 {
        super::X402_V2
    }

    /// Returns the trusted binding to the originally requested resource.
    #[must_use]
    pub const fn resource(&self) -> &ResourceBinding {
        &self.resource
    }

    /// Returns all syntactically valid advertised payment requirements.
    ///
    /// A later policy step must select an explicitly supported requirement.
    #[must_use]
    pub fn requirements(&self) -> &[PaymentRequirement] {
        &self.requirements
    }

    /// Returns opaque, untrusted protocol extensions advertised by the server.
    ///
    /// Presence here does not mean an extension is supported or authorized.
    #[must_use]
    pub const fn untrusted_extensions(&self) -> Option<&Map<String, Value>> {
        self.untrusted_extensions.as_ref()
    }
}

/// Trusted canonical identity for the original protected HTTP resource.
///
/// No `Debug` implementation is provided because a resource URL may contain a
/// sensitive query string.
#[derive(Clone, Eq, PartialEq)]
pub struct ResourceBinding {
    pub(super) canonical_url: String,
    pub(super) origin: String,
}

impl ResourceBinding {
    /// Returns the canonical original resource URL.
    #[must_use]
    pub fn canonical_url(&self) -> &str {
        &self.canonical_url
    }

    /// Returns the canonical origin of the original resource URL.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }
}

/// One syntactically validated x402 v2 payment requirement.
///
/// Validation here establishes canonical fields but does not make the scheme,
/// network, asset, recipient or extensions supported. That is policy work.
#[derive(Clone, PartialEq)]
pub struct PaymentRequirement {
    pub(super) scheme: String,
    pub(super) network: String,
    pub(super) network_namespace_len: usize,
    pub(super) amount: String,
    pub(super) asset: String,
    pub(super) recipient: String,
    pub(super) max_timeout_seconds: u64,
    pub(super) expires_at_unix_seconds: u64,
    pub(super) untrusted_extra: Option<Map<String, Value>>,
}

impl PaymentRequirement {
    /// Returns the advertised payment scheme identifier.
    #[must_use]
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Returns the validated, case-sensitive CAIP-2 network identifier.
    #[must_use]
    pub fn network(&self) -> &str {
        &self.network
    }

    /// Returns the CAIP-2 namespace.
    #[must_use]
    pub fn network_namespace(&self) -> &str {
        &self.network[..self.network_namespace_len]
    }

    /// Returns the CAIP-2 reference.
    #[must_use]
    pub fn network_reference(&self) -> &str {
        &self.network[self.network_namespace_len + 1..]
    }

    /// Returns the exact positive amount in atomic token units.
    #[must_use]
    pub fn amount(&self) -> &str {
        &self.amount
    }

    /// Returns the validated asset identifier.
    ///
    /// EVM addresses are normalized to lowercase. A later policy step must bind
    /// this value to trusted network-specific asset metadata.
    #[must_use]
    pub fn asset(&self) -> &str {
        &self.asset
    }

    /// Returns the validated payment recipient (`payTo`).
    ///
    /// EVM addresses are normalized to lowercase.
    #[must_use]
    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    /// Returns the server-advertised maximum payment completion window.
    #[must_use]
    pub const fn max_timeout_seconds(&self) -> u64 {
        self.max_timeout_seconds
    }

    /// Returns the local fail-closed deadline derived at receipt time.
    ///
    /// x402 v2 advertises `maxTimeoutSeconds`, not an absolute challenge
    /// expiry. This value is the trusted receipt timestamp plus that duration.
    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    /// Returns whether the locally derived deadline has been reached.
    ///
    /// Callers must supply trusted host time. The deadline itself is expired so
    /// an authorization cannot begin at the last permissible second.
    #[must_use]
    pub const fn is_expired_at(&self, now_unix_seconds: u64) -> bool {
        now_unix_seconds >= self.expires_at_unix_seconds
    }

    /// Returns opaque, untrusted scheme-specific requirement data.
    ///
    /// Presence here does not mean any field is supported or authorized.
    #[must_use]
    pub const fn untrusted_extra(&self) -> Option<&Map<String, Value>> {
        self.untrusted_extra.as_ref()
    }
}

/// Typed failure while parsing and validating an x402 v2 challenge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PaymentRequiredParseError {
    /// The response was not HTTP 402 Payment Required.
    WrongStatus,
    /// The canonical `PAYMENT-REQUIRED` header was absent.
    MissingHeader,
    /// More than one `PAYMENT-REQUIRED` header was supplied.
    DuplicateHeader,
    /// The encoded header exceeded the explicit input limit.
    EncodedHeaderLimit,
    /// The header was not canonical standard Base64, with or without padding.
    InvalidBase64,
    /// The decoded challenge was empty or exceeded the explicit input limit.
    DecodedChallengeLimit,
    /// The decoded JSON did not match the closed x402 v2 response schema.
    MalformedChallenge,
    /// The challenge declared a protocol version other than v2.
    UnsupportedVersion,
    /// No payment requirement was advertised.
    MissingRequirements,
    /// More payment requirements were advertised than the parser permits.
    RequirementLimit,
    /// Two advertised requirements were semantically identical.
    DuplicateRequirement,
    /// The original trusted resource URL was not a canonicalizable HTTP URL.
    InvalidOriginalResource,
    /// The challenge resource URL was not a canonicalizable HTTP URL.
    InvalidAdvertisedResource,
    /// The challenge attempted to substitute another resource origin.
    ResourceOriginMismatch,
    /// The challenge described a different URL on the same origin.
    ResourceUrlMismatch,
    /// A payment scheme identifier was malformed.
    InvalidScheme,
    /// A network identifier was not valid CAIP-2 syntax.
    InvalidNetwork,
    /// The advertised asset identifier was malformed.
    InvalidAsset,
    /// The atomic amount was not a canonical positive EVM uint256.
    InvalidAmount,
    /// The advertised recipient was malformed.
    InvalidRecipient,
    /// The advertised completion timeout was zero.
    InvalidTimeout,
    /// The local expiry could not be represented as a Unix timestamp.
    ExpiryOverflow,
}

impl fmt::Display for PaymentRequiredParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WrongStatus => "x402 response must use HTTP 402",
            Self::MissingHeader => "PAYMENT-REQUIRED header is missing",
            Self::DuplicateHeader => "PAYMENT-REQUIRED header is duplicated",
            Self::EncodedHeaderLimit => "PAYMENT-REQUIRED header exceeds the encoded size limit",
            Self::InvalidBase64 => "PAYMENT-REQUIRED header is not valid standard Base64",
            Self::DecodedChallengeLimit => "decoded x402 challenge exceeds its size limit",
            Self::MalformedChallenge => "decoded x402 challenge is malformed",
            Self::UnsupportedVersion => "x402 protocol version is unsupported",
            Self::MissingRequirements => "x402 challenge advertises no payment requirements",
            Self::RequirementLimit => "x402 challenge advertises too many payment requirements",
            Self::DuplicateRequirement => "x402 challenge contains a duplicate payment requirement",
            Self::InvalidOriginalResource => "original x402 resource URL is invalid",
            Self::InvalidAdvertisedResource => "advertised x402 resource URL is invalid",
            Self::ResourceOriginMismatch => {
                "advertised x402 resource origin does not match the request"
            }
            Self::ResourceUrlMismatch => "advertised x402 resource URL does not match the request",
            Self::InvalidScheme => "x402 payment scheme is malformed",
            Self::InvalidNetwork => "x402 payment network is not a valid CAIP-2 identifier",
            Self::InvalidAsset => "x402 payment asset is malformed",
            Self::InvalidAmount => "x402 payment amount is invalid",
            Self::InvalidRecipient => "x402 payment recipient is malformed",
            Self::InvalidTimeout => "x402 payment timeout must be positive",
            Self::ExpiryOverflow => "x402 payment expiry is outside the supported time range",
        })
    }
}

impl Error for PaymentRequiredParseError {}
