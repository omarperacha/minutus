use std::fmt;

use sha2::{Digest, Sha256};

use crate::CanonicalWebUrl;

/// Initial canonical request contract revision.
pub const REQUEST_CONTRACT_REVISION: &str = "zeta-web-request-1";
/// Fixed outbound-header profile including bounded Markdown retrieval.
pub const HEADER_PROFILE_REVISION: &str = "zeta-web-headers-2";
/// Bounded response policy admitting Markdown through the untrusted text extractor.
pub const RESPONSE_POLICY_REVISION: &str = "zeta-web-response-3";
/// AGENT-004 external-resource scope revision for the initial web policy.
pub const WEB_PERMISSION_SCOPE_REVISION: &str = "zeta-web-permission-1";

/// Closed host method set; ordinary model web retrieval still accepts only GET/HEAD.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebMethod {
    /// Retrieve one static representation.
    Get,
    /// Retrieve response metadata without accepting a response body.
    Head,
    /// Host-composed bounded JSON submission for x402.
    Post,
}

impl WebMethod {
    /// Returns the fixed uppercase wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
        }
    }
}

impl TryFrom<&str> for WebMethod {
    type Error = WebMethodError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "GET" => Ok(Self::Get),
            "HEAD" => Ok(Self::Head),
            _ => Err(WebMethodError),
        }
    }
}

/// Invalid or unsupported web method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebMethodError;

impl fmt::Display for WebMethodError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("web method is unsupported")
    }
}

impl std::error::Error for WebMethodError {}

/// Exact canonical request retained before native outbound review.
///
/// Ordinary retrieval is bodyless GET/HEAD. The dedicated x402 adapter may compose
/// bounded JSON POSTs. Headers, redirects, retries, proxies and credentials are
/// never caller-controlled; the request has no serialisation or debug projection.
#[derive(Clone, Eq, PartialEq)]
pub struct WebRequest {
    method: WebMethod,
    url: CanonicalWebUrl,
    body: Vec<u8>,
}

impl WebRequest {
    /// Creates one immutable request from an already canonical URL.
    #[must_use]
    pub const fn new(method: WebMethod, url: CanonicalWebUrl) -> Self {
        Self {
            method,
            url,
            body: Vec::new(),
        }
    }

    /// Composes a bounded JSON-object POST for the dedicated x402 adapter.
    /// # Errors
    /// Rejects oversized, empty or non-object JSON; no headers or credentials are accepted.
    pub fn json_post(url: CanonicalWebUrl, body: Vec<u8>) -> Result<Self, WebMethodError> {
        if body.len() > 32 * 1024
            || serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .is_none_or(|value| !value.is_object())
        {
            return Err(WebMethodError);
        }
        Ok(Self {
            method: WebMethod::Post,
            url,
            body,
        })
    }

    /// Exact immutable host-composed body; empty for ordinary GET/HEAD requests.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Returns the exact HTTP method.
    #[must_use]
    pub const fn method(&self) -> WebMethod {
        self.method
    }

    /// Returns the complete canonical URL.
    #[must_use]
    pub const fn url(&self) -> &CanonicalWebUrl {
        &self.url
    }

    /// Returns the host-owned identity of the complete fixed request contract.
    ///
    /// This digest binds the method, canonical URL, exact POST body and broker policy revisions.
    /// It is suitable for retaining across an x402 approval without retaining
    /// an independently mutable request object.
    #[must_use]
    pub fn identity_sha256(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hash_field(&mut hasher, REQUEST_CONTRACT_REVISION);
        hash_field(&mut hasher, self.method.as_str());
        hash_field(&mut hasher, self.url.as_str());
        hash_field(&mut hasher, HEADER_PROFILE_REVISION);
        hash_field(&mut hasher, RESPONSE_POLICY_REVISION);
        if self.method == WebMethod::Post {
            hash_field(&mut hasher, "x402-json-body-v1");
            hasher.update(Sha256::digest(&self.body));
        }
        hasher.finalize().into()
    }

    /// Builds host-owned exact permission identity and safe display labels.
    #[must_use]
    pub fn permission_descriptor(&self) -> WebPermissionDescriptor {
        let scope_id = hex_digest(self.identity_sha256());

        let path_segments = self.url.path_segment_count();
        let path_summary = if path_segments == 0 {
            "path: root".to_owned()
        } else {
            format!("path: {path_segments} segments")
        };
        let query_summary = self.url.query_summary().map_or_else(
            || "query: none".to_owned(),
            |pairs| format!("query: present, {pairs} pairs; names and values hidden"),
        );

        WebPermissionDescriptor {
            scope_id,
            scope_revision: WEB_PERMISSION_SCOPE_REVISION,
            resources: vec![
                format!("method: {}", self.method.as_str()),
                format!("origin: {}", self.url.origin()),
                path_summary,
                query_summary,
            ],
        }
    }
}

fn hex_digest(digest: [u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(64);
    for byte in digest {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

/// Host-only AGENT-004 scope identity plus credential-safe UI labels.
///
/// The exact URL is represented only by `scope_id`; resources contain no path
/// segments, query names or query values.
pub struct WebPermissionDescriptor {
    scope_id: String,
    scope_revision: &'static str,
    resources: Vec<String>,
}

impl WebPermissionDescriptor {
    /// Returns the SHA-256 identity of method, canonical URL and broker policy.
    #[must_use]
    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    /// Returns the fixed permission policy revision.
    #[must_use]
    pub const fn scope_revision(&self) -> &'static str {
        self.scope_revision
    }

    /// Returns host-authored, credential-safe labels for trusted UI.
    #[must_use]
    pub fn resources(&self) -> &[String] {
        &self.resources
    }

    /// Consumes the descriptor into its credential-safe labels.
    #[must_use]
    pub fn into_resources(self) -> Vec<String> {
        self.resources
    }
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_le_bytes());
    hasher.update(value.as_bytes());
}
