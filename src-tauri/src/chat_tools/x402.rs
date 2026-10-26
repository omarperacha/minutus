//! Model adapter for bounded human-approved x402 discovery and purchase.

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

use app_core::AppCore;
use ipc_types::{
    Conversation, IpcError, IpcErrorCode, JsonValue, ToolOutcome, ToolResultPart,
    X402ApprovalDecision,
};
use model_runtime::{ParsedToolCall, PromptToolDefinition};
use tauri::Manager as _;
use tauri_plugin_dialog::{DialogExt as _, MessageDialogButtons};
use web_retrieval::WebCancellation;

mod cancellation;
mod transport;
use transport::{Method, Request, Scope, configured_scope};
use x402_client::{
    ActiveX402WalletAccount, X402HttpMethod, parse_payment_required_response,
    prepare_x402_payment_approval, prepare_x402_payment_authorization,
};

use crate::{
    wallet_lifecycle::WalletControl,
    x402_approval::{X402ApprovalControl, review_registered_x402_payment},
};

use super::{filesystem::ipc_json_value, validation::required_string_argument};

const MAX_REASON_BYTES: usize = 512;
const MAX_RESOURCE_INPUT_REVIEW_BYTES: usize = 16 * 1024;
#[cfg(debug_assertions)]
const DEBUG_BASE_URL_ENV: &str = "ZETA_X402_DEBUG_BASE_URL";

mod bazaar;
mod continuation;
mod discovery;
mod discovery_surface;
mod policy_failure;
mod schema;
pub(crate) use continuation::active_task_references;
use continuation::resolve_resource;
pub(crate) use discovery_surface::{discovery_tool_definition, is_discovery_call};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Discover,
    Purchase,
    PurchaseUrl,
}

const DEVELOPMENT_SCOPE_DESCRIPTION: &str = "Public x402 uses exact public HTTPS requests with fresh native URL/body review and separate payment approval. Base mainnet USDC is supported. The localhost Base Sepolia qualification transport is available only in debug builds with an explicit configured scope. Discovery reads metadata and never authorises payment.";

pub(super) fn tool_definition() -> PromptToolDefinition {
    PromptToolDefinition {
        name: "x402.request".to_owned(),
        description: format!(
            "{DEVELOPMENT_SCOPE_DESCRIPTION} Discover or purchase a human-approved x402 v2 resource on Base mainnet (USDC, exact EIP-3009). Use `web.retrieve` to read documentation pages, follow their published JSON discovery URL, then call `discover`. `discover` reads a standard Bazaar JSON discovery document and returns resourceRef, bodyExample, inputSchema, and hostGeneratedFields for each resource. Collect every required user value, then call `purchase` with the exact resourceRef, a complete input object satisfying that schema, and a brief reason. Omit hostGeneratedFields: the host creates them, freezes the discovered HTTP request, treats its live 402 as authoritative, requests native approval, signs after fresh user presence, and performs exactly one identical paid retry. `purchase_url` purchases one exact user-supplied bodyless protected-resource URL. Public paid content includes type, provenance, contentComplete and truncation flags. A truncated JSON text is a fragment, not a complete JSON document; never infer missing content or purchase again to recover it automatically. Never substitute a URL for resourceRef, invent user data, or retry an ambiguous outcome."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["discover", "purchase", "purchase_url"],
                    "description": "discover reads Bazaar metadata; purchase executes a discovered resourceRef; purchase_url executes one exact operator-supplied bodyless diagnostic URL."
                },
                "reason": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_REASON_BYTES,
                    "description": "Brief reason shown as untrusted agent context during payment review."
                },
                "input": {
                    "type": ["object", "null"],
                    "description": "For purchase, all model-supplied body fields required by the discovered inputSchema. Omit hostGeneratedFields. Omit for discover and purchase_url."
                },
                "resourceRef": {
                    "type": "string",
                    "pattern": "^ref:x402-resource-[a-f0-9]{32}$",
                    "description": "Exact opaque resource reference returned by successful Bazaar discovery. Required for purchase only."
                },
                "url": {
                    "type": "string",
                    "description": "Exact operator-supplied URL. Required for discover and purchase_url; omit for purchase."
                }
            },
            "required": ["operation", "reason"],
            "oneOf": [
                {
                    "properties": {"operation": {"const": "discover"}},
                    "required": ["url"]
                },
                {
                    "properties": {"operation": {"const": "purchase"}},
                    "required": ["resourceRef", "input"]
                },
                {
                    "properties": {"operation": {"const": "purchase_url"}},
                    "required": ["url"]
                }
            ],
            "additionalProperties": false
        }),
    }
}
pub(super) fn validate_call(call: &ParsedToolCall) -> Result<(), IpcError> {
    let scope = configured_scope()?;
    validate_call_in_scope(call, &scope)
}

fn validate_call_in_scope(call: &ParsedToolCall, scope: &Scope) -> Result<(), IpcError> {
    if call.arguments.contains_key("request") || call.arguments.contains_key("quote") {
        return Err(IpcError::invalid_request(
            "The obsolete Flowzed quote contract is no longer accepted. Run Bazaar discovery, then call `purchase` with `resourceRef` and `input`; the host generates fields declared by `hostGeneratedFields`",
        ));
    }
    let operation = operation(call)?;
    let reason = required_string_argument(&call.arguments, "reason")?;
    if reason.is_empty() || reason.len() > MAX_REASON_BYTES || reason.chars().any(char::is_control)
    {
        return Err(IpcError::invalid_request(
            "The x402 request reason is invalid",
        ));
    }
    match operation {
        Operation::Discover | Operation::PurchaseUrl => {
            reject_present(call, &["input", "resourceRef"])?;
            validate_url(call, scope)
        }
        Operation::Purchase => {
            reject_present(call, &["url"])?;
            let reference = required_string_argument(&call.arguments, "resourceRef")?;
            if !bazaar::valid_resource_reference(reference) {
                return Err(IpcError::invalid_request(
                    "`resourceRef` must be copied exactly from a successful x402 Bazaar discovery result",
                ));
            }
            let input = call.arguments.get("input").ok_or_else(|| {
                IpcError::invalid_request(
                    "A discovered purchase requires `input` satisfying the inputSchema returned by discovery",
                )
            })?;
            if !input.is_object() {
                return Err(IpcError::invalid_request(
                    "The discovered resource `input` must be an object",
                ));
            }
            Ok(())
        }
    }
}
pub(super) fn execute(
    app: &tauri::AppHandle,
    core: &AppCore,
    conversation: &Conversation,
    call: &ParsedToolCall,
    cancelled: &AtomicBool,
) -> Result<ToolOutcome, IpcError> {
    let scope = configured_scope()?;
    validate_call_in_scope(call, &scope)?;
    if cancelled.load(Ordering::Acquire) {
        return Err(cancelled_error());
    }
    let operation = operation(call)?;
    let reason = required_string_argument(&call.arguments, "reason")?;
    let network_cancellation = WebCancellation::default();
    cancellation::run(cancelled, &network_cancellation, || match operation {
        Operation::Discover => {
            let url = required_string_argument(&call.arguments, "url")?;
            tauri::async_runtime::block_on(discovery::execute(
                app,
                &scope,
                url,
                &call.call_id,
                &network_cancellation,
            ))
        }
        Operation::Purchase => {
            let reference = required_string_argument(&call.arguments, "resourceRef")?;
            let resource = resolve_resource(conversation, reference, &scope).ok_or_else(|| {
                correctable(
                    "The x402 resource reference is unknown, expired, or belongs to another conversation. Run discovery again; no payment was attempted",
                )
            })?;
            let input = call.arguments.get("input").ok_or_else(|| {
                correctable("The discovered resource input is missing; no payment was attempted")
            })?;
            let body = resource.materialize_body(input).map_err(|error| {
                correctable(format!(
                    "The discovered resource input is invalid at `{}`: {}; no payment was attempted",
                    input_argument_path(&error.path),
                    error.message
                ))
            })?;
            let request = Request::new(&scope, resource.method(), resource.url(), body)
                .map_err(|_| correctable("The discovered Bazaar request could not be composed; no payment was attempted"))?;
            if matches!(&request, Request::Public(_)) {
                request.review(app)?;
            } else {
                review_resource_input(app, input)?;
            }
            execute_purchase(
                app,
                core,
                &scope,
                &request,
                reason,
                cancelled,
                &network_cancellation,
            )
        }
        Operation::PurchaseUrl => {
            let url = required_string_argument(&call.arguments, "url")?;
            let request = Request::new(&scope, Method::Get, url, Vec::new()).map_err(|_| {
                correctable("The x402 resource request is unavailable; no payment was attempted")
            })?;
            execute_purchase(
                app,
                core,
                &scope,
                &request,
                reason,
                cancelled,
                &network_cancellation,
            )
        }
    })
}

fn execute_purchase(
    app: &tauri::AppHandle,
    core: &AppCore,
    scope: &Scope,
    request: &Request,
    reason: &str,
    cancelled: &AtomicBool,
    network_cancellation: &WebCancellation,
) -> Result<ToolOutcome, IpcError> {
    if !matches!(request.method(), Method::Post) {
        request.review(app)?;
    }
    let wallet = app.state::<WalletControl>();
    let account =
        tauri::async_runtime::block_on(wallet.ensure_x402_account_ready(core, scope.network()))?;
    if cancelled.load(Ordering::Acquire) {
        return Err(cancelled_error());
    }
    tauri::async_runtime::block_on(async {
        if cancelled.load(Ordering::Acquire) {
            network_cancellation.cancel();
            return Err(cancelled_error());
        }
        purchase(
            app,
            core,
            &wallet,
            &account,
            scope,
            request,
            reason,
            network_cancellation,
        )
        .await
    })
}
fn validate_url(call: &ParsedToolCall, scope: &Scope) -> Result<(), IpcError> {
    let url = required_string_argument(&call.arguments, "url")?;
    if scope.admits(url) {
        Ok(())
    } else {
        Err(IpcError::invalid_request(
            "The x402 URL is outside the permitted HTTPS or configured debug scope",
        ))
    }
}

fn reject_present(call: &ParsedToolCall, names: &[&str]) -> Result<(), IpcError> {
    let present = names
        .iter()
        .filter(|name| call.arguments.contains_key(**name))
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>();
    if present.is_empty() {
        Ok(())
    } else {
        Err(IpcError::invalid_request(format!(
            "Remove {} from this x402 operation; use exactly the advertised branch",
            present.join(", ")
        )))
    }
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the single payment boundary keeps wallet freshness, challenge admission, native approval and one paid retry auditable together"
)]
async fn purchase(
    app: &tauri::AppHandle,
    core: &AppCore,
    wallet: &WalletControl,
    account: &ActiveX402WalletAccount,
    scope: &Scope,
    request: &Request,
    reason: &str,
    cancellation: &WebCancellation,
) -> Result<ToolOutcome, IpcError> {
    let ready_before_challenge = wallet
        .ensure_x402_account_ready(core, scope.network())
        .await?;
    if &ready_before_challenge != account {
        return Err(terminal(
            "The selected wallet changed while preparing the x402 request",
        ));
    }
    let unpaid = request.execute(cancellation).await.map_err(|_| {
        terminal("The x402 resource is unavailable or returned an invalid response")
    })?;
    // Wallet re-authentication may wait for the user; it must not restart the challenge lifetime.
    let received_at = unix_now()?;
    if unpaid.status() != 402 {
        return Err(terminal(
            "The resource did not return an x402 payment challenge",
        ));
    }
    let headers = unpaid
        .payment_required_headers()
        .iter()
        .map(Vec::as_slice)
        .collect::<Vec<_>>();
    let refreshed = wallet
        .ensure_x402_account_ready(core, scope.network())
        .await?;
    if &refreshed != account {
        return Err(terminal(
            "The selected wallet changed while preparing the x402 payment",
        ));
    }
    if cancellation.is_cancelled() {
        return Err(cancelled_error());
    }
    let now = unix_now()?;
    let challenge =
        parse_payment_required_response(unpaid.status(), &headers, request.url(), received_at)
            .map_err(|_| terminal("The x402 challenge failed strict v2 validation"))?;
    if challenge.requirements().len() != 1 {
        return Err(terminal(
            "The x402 resource must offer exactly one payment requirement",
        ));
    }
    let requirement = scope
        .admit(&challenge, now)
        .map_err(policy_failure::after_unpaid_challenge)?;
    if account.network() != requirement.network() {
        return Err(terminal(
            "The selected wallet does not match the payment network",
        ));
    }
    let method = match request.method() {
        Method::Get => X402HttpMethod::Get,
        Method::Post => X402HttpMethod::Post,
        Method::Head => X402HttpMethod::Head,
    };
    let candidate = prepare_x402_payment_authorization(
        &requirement,
        method,
        request.identity_sha256(),
        Some(account),
        now,
    )
    .map_err(|_| terminal("The x402 payment authorization could not be prepared"))?;
    let approval = prepare_x402_payment_approval(
        &candidate,
        &format!("x402-session-{}", uuid::Uuid::now_v7()),
        Some(reason),
        now,
    )
    .map_err(|_| terminal("The x402 payment approval could not be prepared"))?;
    let control = app.state::<X402ApprovalControl>();
    let review_id = control.register(approval)?;
    let review = review_registered_x402_payment(app, core, &control, review_id.clone()).await?;
    if review.decision != X402ApprovalDecision::Approved {
        return Err(IpcError {
            code: IpcErrorCode::PermissionDenied,
            message: "The x402 payment was not approved".to_owned(),
            retryable: false,
        });
    }
    let (settlement, extracted) = request
        .paid(core, wallet, &control, &review_id, cancellation)
        .await
        .map_err(|error| execution_failure(&error))?;
    let content = ipc_json_value(&extracted)?;
    Ok(ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: JsonValue::Object(BTreeMap::from([
                ("content".to_owned(), content),
                (
                    "network".to_owned(),
                    JsonValue::String(settlement.network().to_owned()),
                ),
                (
                    "payer".to_owned(),
                    JsonValue::String(settlement.payer().to_owned()),
                ),
                ("paymentAttempted".to_owned(), JsonValue::Bool(true)),
                ("paymentConfirmed".to_owned(), JsonValue::Bool(true)),
                (
                    "transaction".to_owned(),
                    JsonValue::String(settlement.transaction().to_owned()),
                ),
            ])),
        }],
    })
}

fn review_resource_input(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> Result<(), IpcError> {
    if app
        .try_state::<tauri_plugin_dialog::Dialog<tauri::Wry>>()
        .is_none()
    {
        return Err(IpcError::permission_denied(
            "Resource input review needs interactive approval, which is unavailable in this session",
        ));
    }
    let encoded = crate::web_approval::review_json(input);
    if encoded.len() > MAX_RESOURCE_INPUT_REVIEW_BYTES {
        return Err(IpcError {
            code: IpcErrorCode::PayloadTooLarge,
            message: "The x402 resource input is too large to review".to_owned(),
            retryable: false,
        });
    }
    let approved = app
        .dialog()
        .message(format!(
            "Review the exact resource input before Minutus requests payment approval:\n\n{encoded}\n\nNo payment is signed by this step."
        ))
        .title("Review x402 resource input")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Continue to payment review".to_owned(),
            "Cancel".to_owned(),
        ))
        .blocking_show();
    if approved {
        Ok(())
    } else {
        Err(IpcError::permission_denied(
            "The x402 resource input was not approved for payment review",
        ))
    }
}
fn operation(call: &ParsedToolCall) -> Result<Operation, IpcError> {
    match required_string_argument(&call.arguments, "operation")? {
        "discover" => Ok(Operation::Discover),
        "purchase" => Ok(Operation::Purchase),
        "purchase_url" => Ok(Operation::PurchaseUrl),
        _ => Err(IpcError::invalid_request("The x402 operation is invalid")),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn unix_now() -> Result<u64, IpcError> {
    u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp())
        .map_err(|_| IpcError::internal())
}

fn terminal(message: &str) -> IpcError {
    IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: message.to_owned(),
        retryable: false,
    }
}

fn correctable(message: impl Into<String>) -> IpcError {
    IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: message.into(),
        retryable: true,
    }
}

fn input_argument_path(schema_path: &str) -> String {
    schema_path
        .strip_prefix('$')
        .map_or_else(|| "$.input".to_owned(), |suffix| format!("$.input{suffix}"))
}

fn execution_failure(error: &crate::x402_approval::X402ExecutionError) -> IpcError {
    let code = error.code();
    if error.payment_may_have_occurred() {
        terminal(&format!(
            "The x402 payment outcome is ambiguous ({code}); no retry was attempted. \
             Payment and order creation may already have succeeded. Do not repeat the \
             purchase or offer resubmission as a recovery action. Verify the existing \
             transaction or order through a merchant status check or receipt instead."
        ))
    } else {
        terminal(&format!(
            "The x402 payment failed ({code}); no automatic retry was attempted"
        ))
    }
}

fn cancelled_error() -> IpcError {
    IpcError {
        code: IpcErrorCode::Cancelled,
        message: "The x402 request was cancelled".to_owned(),
        retryable: false,
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use crate::x402_approval::{X402AmbiguousFailure, X402ExecutionError};

    pub(super) const BASE_URL: &str = "http://127.0.0.1:5001/test-project/us-central1/api/x402";
    const DISCOVERY_URL: &str =
        "http://127.0.0.1:5001/test-project/us-central1/api/x402/discovery/resources";

    fn call(arguments: BTreeMap<String, serde_json::Value>) -> ParsedToolCall {
        ParsedToolCall {
            call_id: "x402-test".to_owned(),
            recipient: "x402.request".to_owned(),
            name: "x402.request".to_owned(),
            arguments,
        }
    }

    fn base_arguments(operation: &str) -> BTreeMap<String, serde_json::Value> {
        BTreeMap::from([
            ("operation".to_owned(), serde_json::json!(operation)),
            ("reason".to_owned(), serde_json::json!("integration test")),
        ])
    }

    #[test]
    fn operation_shapes_are_distinct_and_bounded() {
        let mut discovery = base_arguments("discover");
        discovery.insert("url".to_owned(), serde_json::json!(DISCOVERY_URL));
        assert!(validate_call(&call(discovery)).is_ok());

        let mut purchase = base_arguments("purchase");
        purchase.insert(
            "resourceRef".to_owned(),
            serde_json::json!("ref:x402-resource-0123456789abcdef0123456789abcdef"),
        );
        purchase.insert("input".to_owned(), serde_json::json!({"query": "value"}));
        assert!(validate_call(&call(purchase)).is_ok());

        let mut direct = base_arguments("purchase_url");
        direct.insert("url".to_owned(), serde_json::json!(DISCOVERY_URL));
        assert!(validate_call(&call(direct)).is_ok());
    }

    #[test]
    fn discovered_purchase_cannot_fall_through_to_direct_url_branch() {
        let mut purchase = base_arguments("purchase");
        purchase.insert("input".to_owned(), serde_json::json!({"query": "value"}));
        purchase.insert("url".to_owned(), serde_json::json!(DISCOVERY_URL));
        assert!(validate_call(&call(purchase)).is_err());
    }

    #[test]
    fn advertised_contract_exposes_generic_resource_input_not_private_fields()
    -> Result<(), &'static str> {
        let definition = tool_definition();
        let properties = definition.parameters["properties"]
            .as_object()
            .ok_or("properties")?;
        assert!(properties.contains_key("input"));
        assert!(properties.contains_key("resourceRef"));
        assert!(!properties.contains_key("quote"));
        assert!(!properties.contains_key("requestId"));
        assert!(!properties.contains_key("accessToken"));
        Ok(())
    }

    #[test]
    fn discovered_schema_paths_are_reported_at_the_tool_input_argument() {
        assert_eq!(
            input_argument_path("$.quote.shipping.country"),
            "$.input.quote.shipping.country"
        );
        assert_eq!(input_argument_path("$"), "$.input");
    }

    #[test]
    fn payment_failure_projects_bounded_host_code() {
        let failure = execution_failure(&X402ExecutionError::SigningFailed);
        assert_eq!(failure.code, ipc_types::IpcErrorCode::InvalidRequest);
        assert!(failure.message.contains("signing_failed"));
        assert!(!failure.retryable);
    }

    #[test]
    fn ambiguous_failure_projects_bounded_host_code_without_retry() {
        let failure = execution_failure(&X402ExecutionError::Ambiguous(
            X402AmbiguousFailure::TransportLoss,
        ));
        assert!(failure.message.contains("ambiguous"));
        assert!(failure.message.contains("no retry was attempted"));
        assert!(
            failure
                .message
                .contains("Do not repeat the purchase or offer resubmission")
        );
        assert!(failure.message.contains("merchant status check or receipt"));
        assert!(!failure.retryable);
    }
}
