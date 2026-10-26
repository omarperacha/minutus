//! Public HTTPS request retention; loopback compatibility exists only in debug builds.

use chain_evm::EvmNetwork;
use ipc_types::IpcError;
use web_retrieval::{
    CanonicalWebUrl, SystemResolver, WebBroker, WebBrokerError, WebBrokerResponse, WebCancellation,
    WebMethod, WebRequest,
};
#[cfg(debug_assertions)]
use web_retrieval::{
    DebugX402Broker, DebugX402Method, DebugX402Request, DebugX402Response, DebugX402Scope,
};
pub(super) use x402_client::X402HttpMethod as Method;

pub(super) enum Scope {
    Public,
    #[cfg(debug_assertions)]
    Local(DebugX402Scope),
}

impl Scope {
    #[cfg(all(test, debug_assertions))]
    pub(super) fn parse(value: &str) -> Result<Self, WebBrokerError> {
        #[cfg(debug_assertions)]
        if value.starts_with("http:") {
            return DebugX402Scope::parse(value).map(Self::Local);
        }
        if Self::Public.admits(value) {
            Ok(Self::Public)
        } else {
            Err(WebBrokerError::ClientUnavailable)
        }
    }
    pub(super) fn admits(&self, url: &str) -> bool {
        match self {
            Self::Public => CanonicalWebUrl::parse(url).is_ok_and(|canonical| {
                // Payments require a domain, not an IP literal, and preserve exact wire spelling.
                let host = canonical.host();
                canonical.as_str() == url
                    && host.contains('.')
                    && host.parse::<std::net::IpAddr>().is_err()
            }),
            #[cfg(debug_assertions)]
            Self::Local(scope) => scope.admits(url),
        }
    }
    pub(super) const fn network(&self) -> EvmNetwork {
        match self {
            Self::Public => EvmNetwork::BaseMainnet,
            #[cfg(debug_assertions)]
            Self::Local(_) => EvmNetwork::BaseSepolia,
        }
    }
    pub(super) fn admit(
        &self,
        challenge: &x402_client::PaymentRequiredChallenge,
        now: u64,
    ) -> Result<x402_client::AdmittedPaymentRequirement, x402_client::X402PolicyError> {
        match self {
            Self::Public => {
                let requirement = x402_client::admit_payment_requirement(challenge, 0, now)?;
                if requirement.network() != EvmNetwork::BaseMainnet {
                    return Err(x402_client::X402PolicyError::UnsupportedNetwork);
                }
                Ok(requirement)
            }
            #[cfg(debug_assertions)]
            Self::Local(scope) => x402_client::admit_debug_loopback_payment_requirement(
                challenge,
                0,
                now,
                scope.origin(),
            ),
        }
    }
}

#[cfg_attr(
    not(debug_assertions),
    expect(
        clippy::unnecessary_wraps,
        reason = "the shared adapter keeps one fallible API because debug scope configuration must validate and fail closed"
    )
)]
pub(super) fn configured_scope() -> Result<Scope, IpcError> {
    #[cfg(debug_assertions)]
    {
        let configured = std::env::var(super::DEBUG_BASE_URL_ENV)
            .ok()
            .or_else(|| option_env!("ZETA_X402_DEBUG_BASE_URL").map(str::to_owned));
        #[cfg(test)]
        let configured = configured.or_else(|| Some(super::tests::BASE_URL.to_owned()));
        if let Some(value) = configured {
            return DebugX402Scope::parse(&value)
                .map(Scope::Local)
                .map_err(|_| {
                    IpcError::invalid_request(
                        "ZETA_X402_DEBUG_BASE_URL must be a canonical loopback URL subtree",
                    )
                });
        }
    }
    Ok(Scope::Public)
}

pub(super) enum Request {
    Public(WebRequest),
    #[cfg(debug_assertions)]
    Local(DebugX402Request),
}
impl Request {
    pub(super) fn new(
        scope: &Scope,
        method: Method,
        url: &str,
        body: Vec<u8>,
    ) -> Result<Self, WebBrokerError> {
        if !scope.admits(url) {
            return Err(WebBrokerError::ClientUnavailable);
        }
        match scope {
            Scope::Public => {
                let url =
                    CanonicalWebUrl::parse(url).map_err(|_| WebBrokerError::ClientUnavailable)?;
                let request = match method {
                    Method::Get if body.is_empty() => WebRequest::new(WebMethod::Get, url),
                    Method::Post => WebRequest::json_post(url, body)
                        .map_err(|_| WebBrokerError::ClientUnavailable)?,
                    _ => return Err(WebBrokerError::ClientUnavailable),
                };
                Ok(Self::Public(request))
            }
            #[cfg(debug_assertions)]
            Scope::Local(scope) => {
                let method = match method {
                    Method::Get => DebugX402Method::Get,
                    Method::Post => DebugX402Method::Post,
                    Method::Head => return Err(WebBrokerError::ClientUnavailable),
                };
                DebugX402Request::new(scope, method, url, body).map(Self::Local)
            }
        }
    }
    pub(super) fn review(&self, app: &tauri::AppHandle) -> Result<(), IpcError> {
        match self {
            Self::Public(request) => {
                if crate::web_approval::review(app, request)? {
                    Ok(())
                } else {
                    Err(IpcError::permission_denied(
                        "The x402 outbound request was not approved",
                    ))
                }
            }
            #[cfg(debug_assertions)]
            Self::Local(_) => Ok(()),
        }
    }
    pub(super) fn url(&self) -> &str {
        match self {
            Self::Public(request) => request.url().as_str(),
            #[cfg(debug_assertions)]
            Self::Local(request) => request.url(),
        }
    }
    pub(super) fn method(&self) -> Method {
        match self {
            Self::Public(request) => match request.method() {
                WebMethod::Get => Method::Get,
                WebMethod::Head => Method::Head,
                WebMethod::Post => Method::Post,
            },
            #[cfg(debug_assertions)]
            Self::Local(request) => match request.method() {
                DebugX402Method::Get => Method::Get,
                DebugX402Method::Post => Method::Post,
            },
        }
    }
    pub(super) fn identity_sha256(&self) -> [u8; 32] {
        match self {
            Self::Public(request) => request.identity_sha256(),
            #[cfg(debug_assertions)]
            Self::Local(request) => request.identity_sha256(None),
        }
    }
    pub(super) async fn execute(
        &self,
        cancellation: &WebCancellation,
    ) -> Result<Response, WebBrokerError> {
        match self {
            Self::Public(request) => WebBroker
                .execute(request, &SystemResolver, cancellation)
                .await
                .map(|response| Response::Public(Box::new(response))),
            #[cfg(debug_assertions)]
            Self::Local(request) => DebugX402Broker
                .execute_request(request, None, None, cancellation)
                .await
                .map(Response::Local),
        }
    }
    pub(super) async fn paid(
        &self,
        core: &app_core::AppCore,
        wallet: &crate::wallet_lifecycle::WalletControl,
        control: &crate::x402_approval::X402ApprovalControl,
        id: &ipc_types::TransactionProposalId,
        cancellation: &WebCancellation,
    ) -> Result<
        (x402_client::ValidatedX402PaymentResponse, serde_json::Value),
        crate::x402_approval::X402ExecutionError,
    > {
        match self {
            Self::Public(request) => {
                let result = control
                    .execute_approved_once(core, wallet, id, request, cancellation)
                    .await?;
                Ok((
                    result.settlement().clone(),
                    app_core::project_web_extraction(result.untrusted_content()),
                ))
            }
            #[cfg(debug_assertions)]
            Self::Local(request) => {
                let result = control
                    .execute_debug_approved_once(core, wallet, id, request, None, cancellation)
                    .await?;
                Ok((
                    result.settlement().clone(),
                    serde_json::from_str(result.untrusted_content()).unwrap_or_else(|_| {
                        serde_json::Value::String(result.untrusted_content().to_owned())
                    }),
                ))
            }
        }
    }
}
pub(super) enum Response {
    Public(Box<WebBrokerResponse>),
    #[cfg(debug_assertions)]
    Local(DebugX402Response),
}
impl Response {
    pub(super) fn status(&self) -> u16 {
        match self {
            Self::Public(r) => r.status(),
            #[cfg(debug_assertions)]
            Self::Local(r) => r.status(),
        }
    }
    pub(super) fn body(&self) -> &[u8] {
        match self {
            Self::Public(r) => r.untrusted_body(),
            #[cfg(debug_assertions)]
            Self::Local(r) => r.body(),
        }
    }
    pub(super) fn payment_required_headers(&self) -> &[Vec<u8>] {
        match self {
            Self::Public(r) => r.payment_required_headers(),
            #[cfg(debug_assertions)]
            Self::Local(r) => r.payment_required_headers(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_scope_rejects_local_and_credentialled_routes_and_never_uses_debug_transport()
    -> Result<(), Box<dyn std::error::Error>> {
        let scope = Scope::Public;
        assert_eq!(scope.network(), EvmNetwork::BaseMainnet);
        for url in [
            "http://127.0.0.1:5001/api",
            "https://localhost/api",
            "https://127.0.0.1/api",
            "https://user:password@example.com/api",
            "https://example.com:444/api",
            "https://example.com/api#fragment",
        ] {
            assert!(!scope.admits(url), "{url}");
            assert!(Request::new(&scope, Method::Get, url, vec![]).is_err());
        }
        let request = Request::new(
            &scope,
            Method::Post,
            "https://example.com/order",
            br#"{"item":"sample"}"#.to_vec(),
        )?;
        assert!(matches!(request, Request::Public(_)));
        assert_eq!(request.method(), Method::Post);
        Ok(())
    }
}
