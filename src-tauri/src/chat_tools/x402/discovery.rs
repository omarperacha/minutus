//! Bazaar JSON discovery and bounded, actionable failure projection.

use super::{
    bazaar, cancelled_error, ipc_json_value, terminal,
    transport::{Method, Request, Response, Scope},
};
use ipc_types::{IpcError, JsonValue, ToolOutcome, ToolResultPart};
use std::collections::BTreeMap;
use web_retrieval::{
    ResponseValidationError, ValidatedWebMediaType, WebBrokerError, WebCancellation,
};

pub(super) async fn execute(
    app: &tauri::AppHandle,
    scope: &Scope,
    url: &str,
    call_id: &str,
    cancellation: &WebCancellation,
) -> Result<ToolOutcome, IpcError> {
    let request = Request::new(scope, Method::Get, url, Vec::new()).map_err(|_| {
        terminal(
            "The discovery URL is outside the supported request scope; no payment was attempted",
        )
    })?;
    request.review(app)?;
    let response = request
        .execute(cancellation)
        .await
        .map_err(transport_error)?;
    let value = discovery_value(&response)?;
    let resources = bazaar::parse_discovery_document(&value, call_id, scope)
        .map_err(|message| terminal(&message))?;
    let model_resources = resources
        .iter()
        .map(bazaar::BazaarResource::model_projection)
        .collect::<Vec<_>>();
    Ok(ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: JsonValue::Object(BTreeMap::from([
                (
                    "operation".to_owned(),
                    JsonValue::String("discovery".to_owned()),
                ),
                (
                    "resources".to_owned(),
                    ipc_json_value(&serde_json::Value::Array(model_resources))?,
                ),
                ("paymentAttempted".to_owned(), JsonValue::Bool(false)),
            ])),
        }],
    })
}
fn discovery_value(response: &Response) -> Result<serde_json::Value, IpcError> {
    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(terminal(&format!(
            "The discovery request returned HTTP {status}; redirects are not followed. This does not establish that x402 is unsupported. Check the published JSON discovery URL. No payment was attempted."
        )));
    }
    let is_json = match response {
        Response::Public(response) => {
            response.provenance().media_type() == Some(ValidatedWebMediaType::Json)
        }
        #[cfg(debug_assertions)]
        Response::Local(_) => true, // The debug response retains only bounded body bytes.
    };
    if !is_json {
        return Err(document_error());
    }
    serde_json::from_slice(response.body()).map_err(|_| terminal(
        "The discovery response was not valid bounded JSON. Check the published JSON discovery endpoint; no payment was attempted."
    ))
}

fn document_error() -> IpcError {
    terminal(
        "Discovery requires a Bazaar JSON document; this response was not supported JSON. If this URL is a documentation page, read it with web.retrieve, follow its published JSON discovery link, then call x402.request with operation=discover at that endpoint. Do not infer that x402 is unavailable from this response. No payment was attempted.",
    )
}

fn transport_error(error: WebBrokerError) -> IpcError {
    let reason = match error {
        WebBrokerError::Cancelled => return cancelled_error(),
        WebBrokerError::DnsDeadline => "DNS resolution timed out",
        WebBrokerError::TargetRejected(_) => "the target was rejected by public-address validation",
        WebBrokerError::ClientUnavailable => "the request client could not be constructed",
        WebBrokerError::ConnectionOrHeaderDeadline => {
            "the connection or response-header deadline elapsed"
        }
        WebBrokerError::BodyIdleDeadline => "the response body stalled",
        WebBrokerError::TotalDeadline => "the total request deadline elapsed",
        WebBrokerError::TransportFailed => "the network or TLS transport failed",
        WebBrokerError::SubmissionRejected => "the host rejected submission",
        WebBrokerError::ResponseRejected(ResponseValidationError::UnsupportedMediaType) => {
            return document_error();
        }
        WebBrokerError::ResponseRejected(_) => {
            "the response failed bounded framing, encoding, size or content validation"
        }
    };
    terminal(&format!(
        "The discovery attempt failed: {reason}. This does not establish that the endpoint is offline or x402 is unsupported. No payment was attempted."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::{
        StatusCode,
        header::{CONTENT_TYPE, HeaderMap, HeaderValue},
    };
    use web_retrieval::{CanonicalWebUrl, WebMethod, test_support::validated_response};

    fn response(
        media: &'static str,
        status: StatusCode,
        body: &[u8],
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(media));
        Ok(Response::Public(Box::new(validated_response(
            WebMethod::Get,
            CanonicalWebUrl::parse("https://example.com/purchasing.md")?,
            status,
            32,
            &headers,
            body,
        )?)))
    }

    #[test]
    fn documentation_has_exact_recovery_guidance_without_claiming_an_outage()
    -> Result<(), Box<dyn std::error::Error>> {
        let error = discovery_value(&response(
            "text/markdown; charset=utf-8",
            StatusCode::OK,
            b"# Purchasing\nFollow https://example.com/discovery/resources",
        )?)
        .err()
        .ok_or("expected rejection")?;
        assert!(error.message.contains("web.retrieve"));
        assert!(error.message.contains("JSON discovery link"));
        assert!(error.message.contains("operation=discover"));
        assert!(error.message.contains("No payment was attempted"));
        assert!(!error.message.contains("endpoint is unavailable"));
        assert!(!error.retryable);
        let json = discovery_value(&response(
            "application/json",
            StatusCode::OK,
            br#"{"items":[]}"#,
        )?)
        .map_err(|_| "JSON rejected")?;
        assert_eq!(json, serde_json::json!({"items":[]}));
        Ok(())
    }

    #[test]
    fn failure_categories_preserve_cancel_status_and_network_uncertainty()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            transport_error(WebBrokerError::Cancelled).code,
            ipc_types::IpcErrorCode::Cancelled
        );
        let timeout = transport_error(WebBrokerError::DnsDeadline);
        assert!(timeout.message.contains("DNS resolution timed out"));
        assert!(
            timeout
                .message
                .contains("does not establish that the endpoint is offline")
        );
        let invalid = discovery_value(&response("application/json", StatusCode::OK, b"{broken")?)
            .err()
            .ok_or("expected JSON error")?;
        assert!(invalid.message.contains("not valid bounded JSON"));
        let missing = discovery_value(&response("text/plain", StatusCode::NOT_FOUND, b"")?)
            .err()
            .ok_or("expected HTTP error")?;
        assert!(missing.message.contains("HTTP 404"));
        assert!(missing.message.contains("No payment was attempted"));
        Ok(())
    }
}
