use std::{fmt, net::IpAddr};

use url::{Host, Url};

use crate::address::is_public_web_address;

/// Maximum encoded URL accepted at the trusted parser boundary.
pub const MAX_WEB_URL_BYTES: usize = 4_096;

/// Failures produced before permission mediation or DNS resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UrlValidationError {
    /// The input is empty or exceeds the fixed byte ceiling.
    InvalidLength,
    /// The URL contains whitespace, controls or ambiguous URL separators.
    AmbiguousSyntax,
    /// The URL parser rejected the absolute URL.
    InvalidSyntax,
    /// Only HTTPS is supported.
    UnsupportedScheme,
    /// The authority has no explicit host.
    MissingHost,
    /// User information is forbidden.
    UserInfoNotAllowed,
    /// URL fragments are forbidden.
    FragmentNotAllowed,
    /// Only the absent/default HTTPS port is supported.
    UnsupportedPort,
    /// An alternate or ambiguous numeric IP spelling was supplied.
    AmbiguousIpAddress,
    /// A literal IP address is outside the public web policy.
    NonPublicAddress,
    /// A single-label or special-use DNS name is not a public web host.
    NonPublicDomain,
}

impl fmt::Display for UrlValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidLength => "URL length is outside the allowed range",
            Self::AmbiguousSyntax => "URL syntax is ambiguous",
            Self::InvalidSyntax => "URL syntax is invalid",
            Self::UnsupportedScheme => "URL scheme is unsupported",
            Self::MissingHost => "URL host is missing",
            Self::UserInfoNotAllowed => "URL user information is unsupported",
            Self::FragmentNotAllowed => "URL fragments are unsupported",
            Self::UnsupportedPort => "URL port is unsupported",
            Self::AmbiguousIpAddress => "numeric IP address syntax is ambiguous",
            Self::NonPublicAddress => "URL host is not a public address",
            Self::NonPublicDomain => "URL host is not a public DNS name",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for UrlValidationError {}

/// Immutable canonical HTTPS URL admitted before permission mediation.
///
/// The type deliberately has no serialization or debug projection so complete
/// URLs, including query values, do not enter ordinary diagnostics by accident.
#[derive(Clone, Eq, PartialEq)]
pub struct CanonicalWebUrl {
    url: Url,
    origin: String,
    host: String,
    literal_address: Option<IpAddr>,
}

impl CanonicalWebUrl {
    /// Parses and canonicalizes one absolute HTTPS URL without resolving DNS.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed or ambiguous syntax, unsupported URL
    /// components, non-canonical numeric addresses and non-public DNS names.
    pub fn parse(input: &str) -> Result<Self, UrlValidationError> {
        validate_raw_input(input)?;
        let raw_authority = raw_authority(input)?;
        let (raw_host, raw_port) = split_authority(raw_authority)?;
        validate_raw_port(raw_port)?;

        let mut url = Url::parse(input).map_err(|_| UrlValidationError::InvalidSyntax)?;
        if url.scheme() != "https" {
            return Err(UrlValidationError::UnsupportedScheme);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(UrlValidationError::UserInfoNotAllowed);
        }
        if url.fragment().is_some() {
            return Err(UrlValidationError::FragmentNotAllowed);
        }
        if url.port_or_known_default() != Some(443) {
            return Err(UrlValidationError::UnsupportedPort);
        }

        let (host, literal_address) = match url.host() {
            Some(Host::Domain(domain)) => {
                validate_domain(domain)?;
                (domain.to_owned(), None)
            }
            Some(Host::Ipv4(address)) => {
                if raw_host != address.to_string() {
                    return Err(UrlValidationError::AmbiguousIpAddress);
                }
                if !is_public_web_address(IpAddr::V4(address)) {
                    return Err(UrlValidationError::NonPublicAddress);
                }
                (address.to_string(), Some(IpAddr::V4(address)))
            }
            Some(Host::Ipv6(address)) => {
                if !is_public_web_address(IpAddr::V6(address)) {
                    return Err(UrlValidationError::NonPublicAddress);
                }
                (address.to_string(), Some(IpAddr::V6(address)))
            }
            None => return Err(UrlValidationError::MissingHost),
        };

        url.set_port(None)
            .map_err(|()| UrlValidationError::InvalidSyntax)?;
        let normalized = normalize_percent_hex(url.as_str())?;
        url = Url::parse(&normalized).map_err(|_| UrlValidationError::InvalidSyntax)?;
        let origin = match literal_address {
            Some(IpAddr::V6(_)) => format!("https://[{host}]"),
            _ => format!("https://{host}"),
        };

        Ok(Self {
            url,
            origin,
            host,
            literal_address,
        })
    }

    /// Returns the complete canonical URL.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    /// Returns the canonical origin without the default HTTPS port.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Returns the canonical DNS name or unbracketed IP literal used for TLS.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the literal address, or `None` when DNS resolution is required.
    #[must_use]
    pub const fn literal_address(&self) -> Option<IpAddr> {
        self.literal_address
    }

    /// Returns the fixed initial HTTPS port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        443
    }

    pub(crate) fn path_segment_count(&self) -> usize {
        self.url.path_segments().map_or(0, |segments| {
            segments.filter(|segment| !segment.is_empty()).count()
        })
    }

    pub(crate) fn query_summary(&self) -> Option<usize> {
        self.url.query().map(|query| {
            query
                .split('&')
                .filter(|component| !component.is_empty())
                .count()
        })
    }

    pub(crate) fn resolve_display_target(
        &self,
        reference: &str,
    ) -> Result<Self, UrlValidationError> {
        if reference.is_empty()
            || reference.len() > MAX_WEB_URL_BYTES
            || reference
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
            || reference.contains('\\')
        {
            return Err(UrlValidationError::AmbiguousSyntax);
        }
        let mut resolved = self
            .url
            .join(reference)
            .map_err(|_| UrlValidationError::InvalidSyntax)?;
        resolved.set_fragment(None);
        Self::parse(resolved.as_str())
    }
}

fn validate_raw_input(input: &str) -> Result<(), UrlValidationError> {
    if input.is_empty() || input.len() > MAX_WEB_URL_BYTES {
        return Err(UrlValidationError::InvalidLength);
    }
    if input
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
        || input.contains('\\')
    {
        return Err(UrlValidationError::AmbiguousSyntax);
    }
    if input.contains('#') {
        return Err(UrlValidationError::FragmentNotAllowed);
    }
    normalize_percent_hex(input).map(|_| ())
}

fn raw_authority(input: &str) -> Result<&str, UrlValidationError> {
    let scheme_end = input.find(':').ok_or(UrlValidationError::InvalidSyntax)?;
    if !input[..scheme_end].eq_ignore_ascii_case("https") {
        return Err(UrlValidationError::UnsupportedScheme);
    }
    let remainder = input
        .get(scheme_end + 1..)
        .and_then(|value| value.strip_prefix("//"))
        .ok_or(UrlValidationError::InvalidSyntax)?;
    let authority_end = remainder.find(['/', '?']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    if authority.is_empty() {
        return Err(UrlValidationError::MissingHost);
    }
    if authority.contains('@') {
        return Err(UrlValidationError::UserInfoNotAllowed);
    }
    Ok(authority)
}

fn split_authority(authority: &str) -> Result<(&str, Option<&str>), UrlValidationError> {
    if let Some(bracketed) = authority.strip_prefix('[') {
        let close = bracketed
            .find(']')
            .ok_or(UrlValidationError::AmbiguousSyntax)?;
        let host = &bracketed[..close];
        if host.is_empty() || host.contains('%') {
            return Err(UrlValidationError::AmbiguousSyntax);
        }
        let suffix = &bracketed[close + 1..];
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or(UrlValidationError::AmbiguousSyntax)?,
            )
        };
        return Ok((host, port));
    }

    if authority.matches(':').count() > 1 {
        return Err(UrlValidationError::AmbiguousSyntax);
    }
    let (host, port) = authority
        .split_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    if host.is_empty() {
        return Err(UrlValidationError::MissingHost);
    }
    if host.contains('%') {
        return Err(UrlValidationError::AmbiguousSyntax);
    }
    Ok((host, port))
}

fn validate_raw_port(port: Option<&str>) -> Result<(), UrlValidationError> {
    if port.is_none_or(|port| port == "443") {
        Ok(())
    } else {
        Err(UrlValidationError::UnsupportedPort)
    }
}

fn validate_domain(domain: &str) -> Result<(), UrlValidationError> {
    let domain = domain.to_ascii_lowercase();
    if domain.ends_with('.')
        || !domain.contains('.')
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
    {
        return Err(UrlValidationError::NonPublicDomain);
    }
    Ok(())
}

fn normalize_percent_hex(input: &str) -> Result<String, UrlValidationError> {
    let bytes = input.as_bytes();
    let mut normalized = String::with_capacity(input.len());
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position] == b'%' {
            let high = *bytes
                .get(position + 1)
                .ok_or(UrlValidationError::AmbiguousSyntax)?;
            let low = *bytes
                .get(position + 2)
                .ok_or(UrlValidationError::AmbiguousSyntax)?;
            if !high.is_ascii_hexdigit() || !low.is_ascii_hexdigit() {
                return Err(UrlValidationError::AmbiguousSyntax);
            }
            normalized.push('%');
            normalized.push(char::from(high.to_ascii_uppercase()));
            normalized.push(char::from(low.to_ascii_uppercase()));
            position += 3;
            continue;
        }
        let character = input[position..]
            .chars()
            .next()
            .ok_or(UrlValidationError::InvalidSyntax)?;
        normalized.push(character);
        position += character.len_utf8();
    }
    Ok(normalized)
}
