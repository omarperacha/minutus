//! Canonical public HTTPS targets and bounded static web retrieval.
//!
//! This crate owns the privileged network boundary. Model-tool registration,
//! interactive permission mediation and desktop presentation remain in the
//! application composition layer.

mod address;
mod broker;
#[cfg(debug_assertions)]
mod debug_x402;
mod extraction;
mod remote_asset;
mod request;
mod response;
mod search;
mod target;
mod web_url;

pub use address::is_public_web_address;
pub use broker::{
    BODY_IDLE_TIMEOUT, CONNECT_TIMEOUT, DNS_TIMEOUT, RESPONSE_HEADER_TIMEOUT,
    TOTAL_REQUEST_TIMEOUT, WebBroker, WebBrokerError, WebBrokerResponse, WebCancellation,
    WebPaymentSignature,
};
#[cfg(debug_assertions)]
pub use debug_x402::{
    DebugX402Broker, DebugX402Method, DebugX402Request, DebugX402Response, DebugX402Scope,
};
pub use extraction::{
    MAX_HTML_DOM_NODES, MAX_HTML_INPUT_BYTES, MAX_HTML_MARKUP_TOKENS, MAX_WEB_EXTRACTION_BYTES,
    MAX_WEB_LINK_LABEL_BYTES, MAX_WEB_LINKS, MAX_WEB_SECTION_BYTES, MAX_WEB_SECTIONS,
    MAX_WEB_TEXT_OUTPUT_BYTES, MAX_WEB_TITLE_BYTES, WebDisplayLink, WebExtraction,
    WebExtractionError, WebExtractionSource, WebExtractionTruncation, WebPageExtraction,
    WebSectionKind, WebTextExtraction, WebTextSection, extract_web_response,
};
pub use remote_asset::{RemoteImageHandoffError, RemoteImageResponse};
pub use request::{
    HEADER_PROFILE_REVISION, REQUEST_CONTRACT_REVISION, RESPONSE_POLICY_REVISION,
    WEB_PERMISSION_SCOPE_REVISION, WebMethod, WebMethodError, WebPermissionDescriptor, WebRequest,
};
pub use response::{
    MAX_DECODED_BODY_BYTES, MAX_ENCODED_BODY_BYTES, MAX_PAYMENT_ERROR_BYTES,
    MAX_PAYMENT_REQUIRED_CHALLENGE_BYTES, MAX_PAYMENT_REQUIREMENTS,
    MAX_REMOTE_ASSET_DECODED_BODY_BYTES, MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
    MAX_RESPONSE_HEADER_BYTES, MAX_RESPONSE_HEADER_FIELD_BYTES, MAX_RESPONSE_HEADER_FIELDS,
    MAX_RESPONSE_STATUS_LINE_BYTES, ResponseValidationError, ValidatedWebMediaType,
    WebPaymentRequiredChallenge, WebResponseKind, WebResponseProvenance,
};
pub use search::{
    BRAVE_SEARCH_POLICY_REVISION, BraveSearchCredential, MAX_SEARCH_QUERY_BYTES,
    MAX_SEARCH_QUERY_TERMS, MAX_SEARCH_RESULTS, SearchBroker, SearchBrokerError, SearchFreshness,
    SearchPermissionDescriptor, SearchRequest, SearchRequestError, SearchResponse, SearchResult,
};
pub use target::{
    HostResolver, MAX_RESOLVED_ADDRESSES, ResolveError, SystemResolver, TargetValidationError,
    ValidatedConnectionPlan, validate_public_target,
};
pub use web_url::{CanonicalWebUrl, MAX_WEB_URL_BYTES, UrlValidationError};

#[cfg(feature = "test-support")]
pub mod test_support;

#[cfg(test)]
mod broker_tests;
#[cfg(test)]
mod tests;
