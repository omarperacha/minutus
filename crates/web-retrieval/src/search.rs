//! Fixed-provider, bounded web-search discovery without result retrieval.

use std::{
    collections::BTreeSet,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue},
    redirect,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::time::timeout;

use crate::{
    CanonicalWebUrl, HostResolver, TargetValidationError, WebCancellation, validate_public_target,
};

/// Brave provider-policy revision included in permission and retention identity.
pub const BRAVE_SEARCH_POLICY_REVISION: &str = "zeta-brave-search-1";
/// Maximum UTF-8 query bytes accepted by the provider-neutral contract.
pub const MAX_SEARCH_QUERY_BYTES: usize = 400;
/// Maximum whitespace-delimited terms accepted in one query.
pub const MAX_SEARCH_QUERY_TERMS: usize = 50;
/// Maximum result candidates exposed to the model.
pub const MAX_SEARCH_RESULTS: u8 = 10;
const DEFAULT_SEARCH_RESULTS: u8 = 5;
const MAX_SEARCH_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_SEARCH_HEADERS: usize = 64;
const MAX_SEARCH_HEADER_BYTES: usize = 32 * 1024;
const MAX_SEARCH_TITLE_BYTES: usize = 256;
const MAX_SEARCH_SNIPPET_BYTES: usize = 1_024;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

/// Closed provider-neutral freshness choices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchFreshness {
    /// No recency restriction.
    Any,
    /// Results from approximately the last day.
    Day,
    /// Results from approximately the last week.
    Week,
    /// Results from approximately the last month.
    Month,
    /// Results from approximately the last year.
    Year,
}

impl SearchFreshness {
    fn brave_value(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Day => Some("pd"),
            Self::Week => Some("pw"),
            Self::Month => Some("pm"),
            Self::Year => Some("py"),
        }
    }
}

impl TryFrom<&str> for SearchFreshness {
    type Error = SearchRequestError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "any" => Ok(Self::Any),
            "day" => Ok(Self::Day),
            "week" => Ok(Self::Week),
            "month" => Ok(Self::Month),
            "year" => Ok(Self::Year),
            _ => Err(SearchRequestError::InvalidFreshness),
        }
    }
}

/// Exact normalized search request created before interactive approval.
#[derive(Clone, Eq, PartialEq)]
pub struct SearchRequest {
    query: String,
    country: Option<String>,
    search_language: Option<String>,
    freshness: SearchFreshness,
    count: u8,
}

impl SearchRequest {
    /// Validates and normalizes one provider-neutral search request.
    ///
    /// # Errors
    ///
    /// Returns a typed rejection for an empty, excessive, control-bearing or
    /// URL-only query, malformed locale option or unsupported result count.
    pub fn new(
        query: &str,
        country: Option<&str>,
        search_language: Option<&str>,
        freshness: SearchFreshness,
        count: Option<u8>,
    ) -> Result<Self, SearchRequestError> {
        let query = normalize_query(query)?;
        let country = country.map(normalize_country).transpose()?;
        let search_language = search_language.map(normalize_language).transpose()?;
        let count = count.unwrap_or(DEFAULT_SEARCH_RESULTS);
        if count == 0 || count > MAX_SEARCH_RESULTS {
            return Err(SearchRequestError::InvalidCount);
        }
        Ok(Self {
            query,
            country,
            search_language,
            freshness,
            count,
        })
    }

    /// Returns the exact normalized query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns the optional two-letter country.
    #[must_use]
    pub fn country(&self) -> Option<&str> {
        self.country.as_deref()
    }

    /// Returns the optional search language.
    #[must_use]
    pub fn search_language(&self) -> Option<&str> {
        self.search_language.as_deref()
    }

    /// Returns the freshness policy.
    #[must_use]
    pub const fn freshness(&self) -> SearchFreshness {
        self.freshness
    }

    /// Returns the requested result ceiling.
    #[must_use]
    pub const fn count(&self) -> u8 {
        self.count
    }

    /// Builds exact hidden identity plus a bounded trusted approval display.
    #[must_use]
    pub fn permission_descriptor(&self) -> SearchPermissionDescriptor {
        let mut hasher = Sha256::new();
        hash_field(&mut hasher, BRAVE_SEARCH_POLICY_REVISION);
        hash_field(&mut hasher, &self.query);
        hash_field(&mut hasher, self.country.as_deref().unwrap_or(""));
        hash_field(&mut hasher, self.search_language.as_deref().unwrap_or(""));
        hash_field(&mut hasher, freshness_name(self.freshness));
        hash_field(&mut hasher, &self.count.to_string());
        let resources = vec![
            "provider: Brave Search".to_owned(),
            format!("query: “{}”", self.query),
            format!("maximum results: {}", self.count),
            format!("freshness: {}", freshness_name(self.freshness)),
        ];
        SearchPermissionDescriptor {
            scope_id: format!("{:x}", hasher.finalize()),
            resources,
        }
    }

    fn request_digest(&self) -> String {
        self.permission_descriptor().scope_id
    }
}

/// Host-only exact search permission identity and display labels.
pub struct SearchPermissionDescriptor {
    scope_id: String,
    resources: Vec<String>,
}

impl SearchPermissionDescriptor {
    /// Returns the exact query, options and provider-policy digest.
    #[must_use]
    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    /// Returns the fixed search permission policy revision.
    #[must_use]
    pub const fn scope_revision(&self) -> &'static str {
        BRAVE_SEARCH_POLICY_REVISION
    }

    /// Consumes the descriptor into trusted UI labels.
    #[must_use]
    pub fn into_resources(self) -> Vec<String> {
        self.resources
    }
}

/// Invalid provider-neutral search request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchRequestError {
    /// Query is empty after normalization.
    EmptyQuery,
    /// Query exceeds byte or term limits.
    QueryTooLarge,
    /// Query contains forbidden controls.
    InvalidQuery,
    /// Query consists only of a URL rather than search terms.
    UrlOnlyQuery,
    /// Country is not a two-letter ASCII code.
    InvalidCountry,
    /// Search language is not a bounded ASCII language tag.
    InvalidLanguage,
    /// Freshness is outside the closed contract.
    InvalidFreshness,
    /// Requested result count is outside the host ceiling.
    InvalidCount,
}

impl fmt::Display for SearchRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("web search request is invalid")
    }
}

impl std::error::Error for SearchRequestError {}

/// Host-only Brave API credential with no serialization or debug projection.
pub struct BraveSearchCredential(String);

impl BraveSearchCredential {
    /// Validates credential bytes obtained from secure host configuration.
    ///
    /// # Errors
    ///
    /// Returns an error for empty, excessive, whitespace or control-bearing data.
    pub fn new(value: String) -> Result<Self, SearchBrokerError> {
        if value.len() < 16
            || value.len() > 512
            || value
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            return Err(SearchBrokerError::CredentialUnavailable);
        }
        Ok(Self(value))
    }

    fn header_value(&self) -> Result<HeaderValue, SearchBrokerError> {
        HeaderValue::from_str(&self.0).map_err(|_| SearchBrokerError::CredentialUnavailable)
    }
}

/// One bounded untrusted search candidate.
pub struct SearchResult {
    rank: u8,
    url: CanonicalWebUrl,
    display_domain: String,
    title: String,
    snippet: String,
    title_truncated: bool,
    snippet_truncated: bool,
}

impl SearchResult {
    /// Returns the provider-neutral one-based rank.
    #[must_use]
    pub const fn rank(&self) -> u8 {
        self.rank
    }

    /// Returns the canonical public HTTPS candidate.
    #[must_use]
    pub const fn url(&self) -> &CanonicalWebUrl {
        &self.url
    }

    /// Returns the credential-free display domain.
    #[must_use]
    pub fn display_domain(&self) -> &str {
        &self.display_domain
    }

    /// Returns the bounded untrusted title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the bounded untrusted snippet.
    #[must_use]
    pub fn snippet(&self) -> &str {
        &self.snippet
    }

    /// Reports whether the provider title was shortened to the host bound.
    #[must_use]
    pub const fn title_truncated(&self) -> bool {
        self.title_truncated
    }

    /// Reports whether the provider snippet was shortened to the host bound.
    #[must_use]
    pub const fn snippet_truncated(&self) -> bool {
        self.snippet_truncated
    }
}

/// Bounded provider-neutral search response.
pub struct SearchResponse {
    request_digest: String,
    response_digest: String,
    results: Vec<SearchResult>,
    omitted_results: usize,
    searched_at_unix_millis: u64,
}

impl SearchResponse {
    /// Returns exact request-policy identity without query text.
    #[must_use]
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }

    /// Returns the digest of the bounded provider response bytes.
    #[must_use]
    pub fn response_digest(&self) -> &str {
        &self.response_digest
    }

    /// Returns admitted ordered candidates.
    #[must_use]
    pub fn results(&self) -> &[SearchResult] {
        &self.results
    }

    /// Returns the number of provider candidates omitted by validation or limits.
    #[must_use]
    pub const fn omitted_results(&self) -> usize {
        self.omitted_results
    }

    /// Returns host-observed completion time.
    #[must_use]
    pub const fn searched_at_unix_millis(&self) -> u64 {
        self.searched_at_unix_millis
    }
}

/// Sanitized search broker failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchBrokerError {
    /// Host cancellation won.
    Cancelled,
    /// No valid host-only Brave credential was available.
    CredentialUnavailable,
    /// Fixed client construction failed.
    ClientUnavailable,
    /// The fixed provider endpoint did not resolve entirely to public addresses.
    TargetRejected(TargetValidationError),
    /// Connect, response or total deadline expired.
    Deadline,
    /// Brave rejected authentication.
    AuthenticationRejected,
    /// Provider quota or rate limit was reached.
    RateLimited,
    /// A payment-required response is terminal and non-paying.
    PaymentRequired,
    /// Other provider status was terminal.
    ProviderRejected,
    /// Response headers or body exceeded bounds.
    ResponseTooLarge,
    /// Response media or JSON shape was invalid.
    MalformedResponse,
    /// Sanitized HTTP or TLS transport failure.
    TransportFailed,
}

impl fmt::Display for SearchBrokerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "web search was cancelled",
            Self::CredentialUnavailable => "Brave Search is not configured",
            Self::ClientUnavailable => "web search client is unavailable",
            Self::TargetRejected(_) => "web search provider target was rejected",
            Self::Deadline => "web search timed out",
            Self::AuthenticationRejected => "web search authentication was rejected",
            Self::RateLimited => "web search rate limit was reached",
            Self::PaymentRequired => "web search provider requested payment",
            Self::ProviderRejected => "web search provider rejected the request",
            Self::ResponseTooLarge => "web search response exceeded its limit",
            Self::MalformedResponse => "web search response was invalid",
            Self::TransportFailed => "web search transport failed",
        })
    }
}

impl std::error::Error for SearchBrokerError {}

/// Application-owned Brave Search broker.
#[derive(Clone, Copy, Default)]
pub struct SearchBroker;

impl SearchBroker {
    /// Executes one approved query against the fixed Brave JSON endpoint.
    ///
    /// # Errors
    ///
    /// Returns only bounded credential, cancellation, deadline, provider,
    /// response-limit, response-shape or transport outcomes.
    pub async fn execute(
        &self,
        request: &SearchRequest,
        credential: &BraveSearchCredential,
        resolver: &impl HostResolver,
        cancellation: &WebCancellation,
    ) -> Result<SearchResponse, SearchBrokerError> {
        let operation = async {
            if cancellation.is_cancelled() {
                return Err(SearchBrokerError::Cancelled);
            }
            let endpoint = CanonicalWebUrl::parse(BRAVE_ENDPOINT)
                .map_err(|_| SearchBrokerError::ClientUnavailable)?;
            let plan = tokio::select! {
                () = cancellation.cancelled() => return Err(SearchBrokerError::Cancelled),
                plan = validate_public_target(endpoint, resolver) => {
                    plan.map_err(SearchBrokerError::TargetRejected)?
                }
            };
            let addresses = plan.socket_addresses().collect::<Vec<_>>();
            let client = Client::builder()
                .https_only(true)
                .no_proxy()
                .redirect(redirect::Policy::none())
                .retry(reqwest::retry::never())
                .http1_only()
                .min_tls_version(reqwest::tls::Version::TLS_1_2)
                .connect_timeout(Duration::from_secs(5))
                .read_timeout(Duration::from_secs(5))
                .timeout(SEARCH_TIMEOUT)
                .pool_max_idle_per_host(0)
                .referer(false)
                .user_agent("ZetaSearch/1")
                .resolve_to_addrs(plan.server_name(), &addresses)
                .build()
                .map_err(|_| SearchBrokerError::ClientUnavailable)?;
            let body = brave_request_json(request);
            let response = tokio::select! {
                () = cancellation.cancelled() => return Err(SearchBrokerError::Cancelled),
                response = client
                    .post(BRAVE_ENDPOINT)
                    .header(ACCEPT, "application/json")
                    .header("X-Subscription-Token", credential.header_value()?)
                    .json(&body)
                    .send() => response.map_err(|error| {
                        if error.is_timeout() { SearchBrokerError::Deadline } else { SearchBrokerError::TransportFailed }
                    })?,
            };
            let status = response.status();
            validate_status(status)?;
            validate_headers(response.headers(), response.content_length())?;
            let mut response = response;
            let mut bytes = Vec::new();
            loop {
                let chunk = tokio::select! {
                    () = cancellation.cancelled() => return Err(SearchBrokerError::Cancelled),
                    chunk = response.chunk() => chunk.map_err(|error| {
                        if error.is_timeout() { SearchBrokerError::Deadline } else { SearchBrokerError::TransportFailed }
                    })?,
                };
                let Some(chunk) = chunk else { break };
                if bytes.len().saturating_add(chunk.len()) > MAX_SEARCH_RESPONSE_BYTES {
                    return Err(SearchBrokerError::ResponseTooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            parse_response(request, &bytes)
        };
        timeout(SEARCH_TIMEOUT, operation)
            .await
            .map_err(|_| SearchBrokerError::Deadline)?
    }
}

fn brave_request_json(request: &SearchRequest) -> serde_json::Value {
    let mut body = serde_json::Map::from_iter([
        (
            "q".to_owned(),
            serde_json::Value::String(request.query.clone()),
        ),
        ("count".to_owned(), serde_json::Value::from(request.count)),
        (
            "safesearch".to_owned(),
            serde_json::Value::String("strict".to_owned()),
        ),
        ("spellcheck".to_owned(), serde_json::Value::Bool(false)),
    ]);
    if let Some(country) = &request.country {
        body.insert(
            "country".to_owned(),
            serde_json::Value::String(country.clone()),
        );
    }
    if let Some(language) = &request.search_language {
        body.insert(
            "search_lang".to_owned(),
            serde_json::Value::String(language.clone()),
        );
    }
    if let Some(freshness) = request.freshness.brave_value() {
        body.insert(
            "freshness".to_owned(),
            serde_json::Value::String(freshness.to_owned()),
        );
    }
    serde_json::Value::Object(body)
}

fn validate_status(status: StatusCode) -> Result<(), SearchBrokerError> {
    match status.as_u16() {
        200 => Ok(()),
        401 | 403 => Err(SearchBrokerError::AuthenticationRejected),
        402 => Err(SearchBrokerError::PaymentRequired),
        429 => Err(SearchBrokerError::RateLimited),
        _ => Err(SearchBrokerError::ProviderRejected),
    }
}

fn validate_headers(
    headers: &HeaderMap,
    content_length: Option<u64>,
) -> Result<(), SearchBrokerError> {
    if headers.len() > MAX_SEARCH_HEADERS
        || headers
            .iter()
            .try_fold(0_usize, |total, (name, value)| {
                total
                    .checked_add(name.as_str().len())?
                    .checked_add(value.as_bytes().len())
            })
            .is_none_or(|total| total > MAX_SEARCH_HEADER_BYTES)
        || content_length.is_some_and(|length| length > MAX_SEARCH_RESPONSE_BYTES as u64)
    {
        return Err(SearchBrokerError::ResponseTooLarge);
    }
    let json = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
        });
    if !json {
        return Err(SearchBrokerError::MalformedResponse);
    }
    if let Some(value) = headers.get(CONTENT_LENGTH)
        && value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .is_none()
    {
        return Err(SearchBrokerError::MalformedResponse);
    }
    Ok(())
}

#[derive(Deserialize)]
struct BraveEnvelope {
    web: Option<BraveWeb>,
}

#[derive(Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
}

fn parse_response(
    request: &SearchRequest,
    bytes: &[u8],
) -> Result<SearchResponse, SearchBrokerError> {
    let parsed: BraveEnvelope =
        serde_json::from_slice(bytes).map_err(|_| SearchBrokerError::MalformedResponse)?;
    let candidates = parsed.web.map_or_else(Vec::new, |web| web.results);
    let mut omitted_results = 0_usize;
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    for candidate in candidates {
        if results.len() >= usize::from(request.count) {
            omitted_results += 1;
            continue;
        }
        let Ok(url) = CanonicalWebUrl::parse(&candidate.url) else {
            omitted_results += 1;
            continue;
        };
        if !seen.insert(url.as_str().to_owned()) {
            omitted_results += 1;
            continue;
        }
        let Some((title, title_truncated)) = bounded_text(&candidate.title, MAX_SEARCH_TITLE_BYTES)
        else {
            omitted_results += 1;
            continue;
        };
        let (snippet, snippet_truncated) =
            bounded_text(&candidate.description, MAX_SEARCH_SNIPPET_BYTES)
                .unwrap_or_else(|| (String::new(), false));
        let parsed_url =
            url::Url::parse(url.as_str()).map_err(|_| SearchBrokerError::MalformedResponse)?;
        let display_domain = parsed_url
            .host_str()
            .ok_or(SearchBrokerError::MalformedResponse)?
            .to_owned();
        let rank =
            u8::try_from(results.len() + 1).map_err(|_| SearchBrokerError::MalformedResponse)?;
        results.push(SearchResult {
            rank,
            url,
            display_domain,
            title,
            snippet,
            title_truncated,
            snippet_truncated,
        });
    }
    Ok(SearchResponse {
        request_digest: request.request_digest(),
        response_digest: format!("{:x}", Sha256::digest(bytes)),
        results,
        omitted_results,
        searched_at_unix_millis: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            }),
    })
}

fn normalize_query(value: &str) -> Result<String, SearchRequestError> {
    if value.chars().any(char::is_control) {
        return Err(SearchRequestError::InvalidQuery);
    }
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return Err(SearchRequestError::EmptyQuery);
    }
    if normalized.len() > MAX_SEARCH_QUERY_BYTES
        || normalized.split_whitespace().count() > MAX_SEARCH_QUERY_TERMS
    {
        return Err(SearchRequestError::QueryTooLarge);
    }
    if url::Url::parse(&normalized).is_ok() || normalized.starts_with("www.") {
        return Err(SearchRequestError::UrlOnlyQuery);
    }
    Ok(normalized)
}

fn normalize_country(value: &str) -> Result<String, SearchRequestError> {
    if value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        Ok(value.to_ascii_uppercase())
    } else {
        Err(SearchRequestError::InvalidCountry)
    }
}

fn normalize_language(value: &str) -> Result<String, SearchRequestError> {
    if (2..=8).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(SearchRequestError::InvalidLanguage)
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> Option<(String, bool)> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() || normalized.chars().any(char::is_control) {
        return None;
    }
    if normalized.len() <= max_bytes {
        return Some((normalized, false));
    }
    let mut boundary = max_bytes;
    while !normalized.is_char_boundary(boundary) {
        boundary -= 1;
    }
    Some((normalized[..boundary].trim_end().to_owned(), true))
}

const fn freshness_name(value: SearchFreshness) -> &'static str {
    match value {
        SearchFreshness::Any => "any",
        SearchFreshness::Day => "day",
        SearchFreshness::Week => "week",
        SearchFreshness::Month => "month",
        SearchFreshness::Year => "year",
    }
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_be_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_options_are_normalized_and_exactly_bound() -> Result<(), Box<dyn std::error::Error>>
    {
        let request = SearchRequest::new(
            "  current   rust releases ",
            Some("es"),
            Some("EN"),
            SearchFreshness::Week,
            Some(3),
        )?;
        assert_eq!(request.query(), "current rust releases");
        assert_eq!(request.country(), Some("ES"));
        assert_eq!(request.search_language(), Some("en"));
        assert_eq!(request.count(), 3);
        let changed = SearchRequest::new(
            "current rust release",
            Some("es"),
            Some("EN"),
            SearchFreshness::Week,
            Some(3),
        )?;
        assert_ne!(
            request.permission_descriptor().scope_id(),
            changed.permission_descriptor().scope_id()
        );
        Ok(())
    }

    #[test]
    fn unsafe_and_excessive_queries_fail_before_provider_io() {
        for query in [
            "",
            "https://example.com/path",
            "www.example.com",
            "query\u{0000}control",
            "query\ncontrol",
        ] {
            assert!(SearchRequest::new(query, None, None, SearchFreshness::Any, None).is_err());
        }
        assert!(
            SearchRequest::new(
                &"x".repeat(MAX_SEARCH_QUERY_BYTES + 1),
                None,
                None,
                SearchFreshness::Any,
                None
            )
            .is_err()
        );
        assert!(
            SearchRequest::new(
                "valid",
                None,
                None,
                SearchFreshness::Any,
                Some(MAX_SEARCH_RESULTS + 1)
            )
            .is_err()
        );
    }

    #[test]
    fn response_omits_invalid_duplicates_and_bounds_untrusted_text()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = SearchRequest::new("zeta", None, None, SearchFreshness::Any, Some(3))?;
        let bytes = serde_json::to_vec(&serde_json::json!({"web":{"results":[
            {"title":" First result ","url":"https://example.com/a","description":"useful snippet"},
            {"title":"duplicate","url":"https://example.com/a","description":"duplicate"},
            {"title":"private","url":"http://127.0.0.1/a","description":"unsafe"},
            {"title":"Second","url":"https://www.rust-lang.org/learn","description":"x".repeat(2_000)}
        ]}}))?;
        let response = parse_response(&request, &bytes)?;
        assert_eq!(response.results().len(), 2);
        assert_eq!(response.omitted_results(), 2);
        assert_eq!(response.results()[0].rank(), 1);
        assert_eq!(
            response.results()[1].snippet().len(),
            MAX_SEARCH_SNIPPET_BYTES
        );
        assert!(response.results()[1].snippet_truncated());
        assert!(!response.results()[0].title_truncated());
        Ok(())
    }

    #[test]
    fn payment_auth_rate_limit_and_remote_failures_are_closed() {
        assert_eq!(
            validate_status(StatusCode::PAYMENT_REQUIRED),
            Err(SearchBrokerError::PaymentRequired)
        );
        assert_eq!(
            validate_status(StatusCode::UNAUTHORIZED),
            Err(SearchBrokerError::AuthenticationRejected)
        );
        assert_eq!(
            validate_status(StatusCode::TOO_MANY_REQUESTS),
            Err(SearchBrokerError::RateLimited)
        );
        assert_eq!(
            validate_status(StatusCode::INTERNAL_SERVER_ERROR),
            Err(SearchBrokerError::ProviderRejected)
        );
    }
}
