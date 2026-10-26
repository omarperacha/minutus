use std::{
    collections::BTreeSet,
    fmt,
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
};

use crate::{CanonicalWebUrl, is_public_web_address};

/// Maximum DNS answers admitted into one immutable connection plan.
pub const MAX_RESOLVED_ADDRESSES: usize = 32;

/// Sanitized resolver failure that carries no operating-system diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveError {
    /// DNS resolution was unavailable or failed.
    Unavailable,
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DNS resolution is unavailable")
    }
}

impl std::error::Error for ResolveError {}

/// Trusted resolver boundary used after interactive permission.
pub trait HostResolver: Send + Sync {
    /// Resolves one canonical ASCII DNS name without applying URL policy.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when resolution cannot complete.
    fn resolve<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send + 'a>>;
}

/// Operating-system resolver for the production composition layer.
#[derive(Clone, Copy, Default)]
pub struct SystemResolver;

impl HostResolver for SystemResolver {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send + 'a>> {
        Box::pin(async move {
            tokio::net::lookup_host((host, 443))
                .await
                .map(|addresses| {
                    addresses
                        .take(MAX_RESOLVED_ADDRESSES + 1)
                        .map(|address| address.ip())
                        .collect()
                })
                .map_err(|_| ResolveError::Unavailable)
        })
    }
}

/// Failures produced while validating a literal or complete DNS answer set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetValidationError {
    /// DNS lookup failed without exposing resolver diagnostics.
    ResolutionFailed,
    /// DNS returned no usable answer.
    NoAddresses,
    /// DNS returned more answers than one bounded connection plan can hold.
    TooManyAddresses,
    /// At least one literal or resolved answer was not public.
    NonPublicAddress,
}

impl fmt::Display for TargetValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ResolutionFailed => "target resolution failed",
            Self::NoAddresses => "target resolution returned no addresses",
            Self::TooManyAddresses => "target resolution returned too many addresses",
            Self::NonPublicAddress => "target contains a non-public address",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for TargetValidationError {}

/// Immutable DNS-to-connection binding for one canonical approved target.
///
/// The plan has no deserialization or public constructor. A broker consumes its
/// exact socket addresses while retaining `server_name` for TLS SNI and
/// certificate verification; it must not perform another DNS lookup.
#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedConnectionPlan {
    canonical_url: CanonicalWebUrl,
    server_name: String,
    addresses: Vec<SocketAddr>,
}

impl ValidatedConnectionPlan {
    /// Returns the exact canonical URL represented by this plan.
    #[must_use]
    pub const fn canonical_url(&self) -> &CanonicalWebUrl {
        &self.canonical_url
    }

    /// Returns the DNS name or IP literal retained for TLS verification.
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Returns the fixed HTTPS port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        443
    }

    /// Iterates the complete validated socket-address set in stable order.
    #[must_use]
    pub fn socket_addresses(&self) -> impl ExactSizeIterator<Item = SocketAddr> + '_ {
        self.addresses.iter().copied()
    }
}

/// Validates a canonical literal or every answer from one post-approval lookup.
///
/// DNS names are resolved exactly once through the supplied trusted resolver.
/// Any non-public answer rejects the whole set. The returned connection plan
/// binds later connection establishment to the validated addresses.
///
/// # Errors
///
/// Returns a sanitized error when resolution fails, returns nothing or includes
/// any address outside the public web policy.
pub async fn validate_public_target(
    canonical_url: CanonicalWebUrl,
    resolver: &impl HostResolver,
) -> Result<ValidatedConnectionPlan, TargetValidationError> {
    let answers = if let Some(address) = canonical_url.literal_address() {
        vec![address]
    } else {
        resolver
            .resolve(canonical_url.host())
            .await
            .map_err(|_| TargetValidationError::ResolutionFailed)?
    };
    if answers.is_empty() {
        return Err(TargetValidationError::NoAddresses);
    }
    if answers.len() > MAX_RESOLVED_ADDRESSES {
        return Err(TargetValidationError::TooManyAddresses);
    }
    if answers
        .iter()
        .copied()
        .any(|address| !is_public_web_address(address))
    {
        return Err(TargetValidationError::NonPublicAddress);
    }

    let addresses = answers
        .into_iter()
        .map(|address| SocketAddr::new(address, canonical_url.port()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let server_name = canonical_url.host().to_owned();
    Ok(ValidatedConnectionPlan {
        canonical_url,
        server_name,
        addresses,
    })
}
