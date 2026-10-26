//! Permissioned web search, retrieval, authorization, and managed-asset handoff.

mod authorization;
mod extraction;
mod remote_asset;
mod retrieval;
mod search;

pub use authorization::{
    AuthorizedWebRequest, StartedWebRequest, WebAuthorizationError, authorize_web_request,
    web_permission_scope,
};
pub use extraction::project_web_extraction;
pub use remote_asset::{RemoteAssetProvenance, RemoteAssetReceipt};
pub use retrieval::{
    PreparedWebToolResult, WEB_RETRIEVE_TOOL, WebDiagnosticOutcome, WebToolResultError,
    prepare_web_tool_result, validate_web_tool_request, web_retrieval_reuse_digest,
    web_tool_registry,
};
pub use search::{
    AuthorizedSearchRequest, PreparedSearchToolResult, StartedSearchRequest, WEB_SEARCH_TOOL,
    WebSearchAuthorizationError, WebSearchError, authorize_web_search_request,
    prepare_web_search_tool_result, validate_web_search_tool_request, web_search_permission_scope,
    web_search_tool_registry,
};
