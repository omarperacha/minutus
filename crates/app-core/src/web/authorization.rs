//! Exact authorization for permissioned static web requests.

use std::fmt;

use agent_core::{
    ApprovalRequirement, ExecutionAuthorization, ExecutionLedgerError, PermissionResourceKind,
    PermissionResourceScope, ProjectCapability, ToolClassification, ValidatedToolRequest,
};
use web_retrieval::{
    HostResolver, WebBroker, WebBrokerError, WebBrokerResponse, WebCancellation, WebRequest,
};

/// Web-specific exact authorization admission failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebAuthorizationError {
    /// The registry definition is not the interactive external-network tool policy.
    WrongToolPolicy,
    /// The receipt does not cover the request, canonical web scope or project.
    BindingMismatch,
    /// The host-owned scope could not be represented under AGENT-004 bounds.
    InvalidScope,
}

impl fmt::Display for WebAuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::WrongToolPolicy => "tool is not an interactive web retrieval",
            Self::BindingMismatch => "web authorization does not match the request",
            Self::InvalidScope => "web permission scope is invalid",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for WebAuthorizationError {}

/// Builds the exact hidden web scope and credential-safe trusted UI labels.
///
/// # Errors
///
/// Returns an error when the project ID or generated scope violates AGENT-004
/// bounds.
pub fn web_permission_scope(
    project_id: impl Into<String>,
    request: &WebRequest,
) -> Result<PermissionResourceScope, ExecutionLedgerError> {
    let descriptor = request.permission_descriptor();
    let scope_id = descriptor.scope_id().to_owned();
    let scope_revision = descriptor.scope_revision();
    let resources = descriptor.into_resources();
    PermissionResourceScope::new(
        project_id,
        PermissionResourceKind::External,
        scope_id,
        scope_revision,
        resources,
    )
}

/// Non-cloneable exact web request paired with its consumed AGENT-004 receipt.
///
/// This type has no serialization or debug projection. The only operation
/// consumes it into one broker execution.
pub struct AuthorizedWebRequest {
    request: WebRequest,
    authorization: ExecutionAuthorization,
}

impl AuthorizedWebRequest {
    /// Returns the exact request for trusted coordination without releasing the
    /// authorization receipt.
    #[must_use]
    pub const fn request(&self) -> &WebRequest {
        &self.request
    }

    /// Records the exact approved request in the generic agent state machine.
    ///
    /// # Errors
    ///
    /// Returns an error unless that machine is awaiting this request's
    /// interactive approval.
    pub fn start(
        self,
        state: &mut agent_core::AgentStateMachine,
        expected: &agent_core::TransitionToken,
    ) -> Result<StartedWebRequest, agent_core::AgentStateMachineError> {
        let Self {
            request,
            authorization,
        } = self;
        state.approve_and_start_tool(expected, authorization, true)?;
        Ok(StartedWebRequest { request })
    }
}

/// Non-cloneable exact request admitted by both permission and agent state.
pub struct StartedWebRequest {
    request: WebRequest,
}

impl StartedWebRequest {
    /// Consumes the admitted request into one bounded broker execution.
    ///
    /// # Errors
    ///
    /// Returns a sanitized target, cancellation, deadline, policy or transport
    /// failure from the network broker.
    pub async fn execute(
        self,
        broker: &WebBroker,
        resolver: &impl HostResolver,
        cancellation: &WebCancellation,
    ) -> Result<WebBrokerResponse, WebBrokerError> {
        broker.execute(&self.request, resolver, cancellation).await
    }
}

/// Consumes an AGENT-004 receipt into one exact authorized web request.
///
/// # Errors
///
/// Returns an error unless the tool is an interactive external network effect
/// and the receipt matches its call ID, name, original argument digest,
/// capability, project and canonical web scope.
pub fn authorize_web_request(
    project_id: impl Into<String>,
    validated_request: &ValidatedToolRequest,
    request: WebRequest,
    authorization: ExecutionAuthorization,
) -> Result<AuthorizedWebRequest, WebAuthorizationError> {
    let definition = validated_request.definition();
    if definition.classification() != ToolClassification::ExternalSideEffect
        || definition.capability() != ProjectCapability::ExternalNetwork
        || definition.approval() != ApprovalRequirement::Interactive
    {
        return Err(WebAuthorizationError::WrongToolPolicy);
    }
    let scope = web_permission_scope(project_id, &request)
        .map_err(|_| WebAuthorizationError::InvalidScope)?;
    if !authorization.matches_request_and_scope(validated_request, &scope) {
        return Err(WebAuthorizationError::BindingMismatch);
    }
    Ok(AuthorizedWebRequest {
        request,
        authorization,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use agent_core::{
        ApprovalRequirement, LedgerPermissionDecision, PermissionMediator, RiskLevel,
        SupportedResultType, ToolDefinition, ToolLimits, ToolRegistry, UntrustedToolRequest,
    };
    use serde_json::json;
    use web_retrieval::{CanonicalWebUrl, WebMethod};

    use super::*;

    fn registry() -> Result<ToolRegistry, agent_core::ToolRegistryError> {
        ToolRegistry::new([ToolDefinition::new(
            "web.retrieve",
            "Retrieve one approved public static HTTPS resource.",
            json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "minLength": 1, "maxLength": 4096},
                    "method": {"type": "string", "enum": ["GET", "HEAD"]}
                },
                "required": ["url", "method"],
                "additionalProperties": false
            }),
            ToolClassification::ExternalSideEffect,
            ProjectCapability::ExternalNetwork,
            ApprovalRequirement::Interactive,
            RiskLevel::Medium,
            ToolLimits::new(Duration::from_secs(30), 64 * 1024, 1)?,
            BTreeSet::from([SupportedResultType::Json]),
        )?])
    }

    fn validated(
        registry: &ToolRegistry,
        call_id: &str,
        url: &str,
        method: &str,
    ) -> Result<ValidatedToolRequest, agent_core::ToolRegistryError> {
        registry.validate_request(UntrustedToolRequest {
            call_id: call_id.to_owned(),
            name: "web.retrieve".to_owned(),
            arguments: json!({"url": url, "method": method}),
        })
    }

    #[test]
    fn prompt_is_safe_while_receipt_binds_exact_url_method_arguments_and_project()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = registry()?;
        let raw_url = "https://example.com/private-token/report?api_key=secret&x=1";
        let validated = validated(&registry, "call-web-1", raw_url, "GET")?;
        let request = WebRequest::new(WebMethod::Get, CanonicalWebUrl::parse(raw_url)?);
        let scope = web_permission_scope("project-1", &request)?;
        let mut mediator = PermissionMediator::default();
        let prompt = mediator.request_interactive("permission-web-1", &validated, scope.clone())?;

        let rendered = prompt.resources.join("\n");
        assert!(rendered.contains("method: GET"));
        assert!(rendered.contains("origin: https://example.com"));
        assert!(rendered.contains("path: 2 segments"));
        assert!(rendered.contains("query: present, 2 pairs"));
        assert!(!rendered.contains("private-token"));
        assert!(!rendered.contains("api_key"));
        assert!(!rendered.contains("secret"));

        let receipt = mediator.decide(
            "permission-web-1",
            &validated,
            &scope,
            LedgerPermissionDecision::Approved,
        )?;
        let authorized = authorize_web_request(
            "project-1",
            &validated,
            request,
            receipt.authorization.ok_or("approval receipt missing")?,
        )?;
        assert_eq!(authorized.request().method(), WebMethod::Get);
        Ok(())
    }

    #[test]
    fn changed_canonical_request_cannot_use_an_existing_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = registry()?;
        let raw_url = "https://example.com/report";
        let validated = validated(&registry, "call-web-1", raw_url, "GET")?;
        let approved = WebRequest::new(WebMethod::Get, CanonicalWebUrl::parse(raw_url)?);
        let scope = web_permission_scope("project-1", &approved)?;
        let mut mediator = PermissionMediator::default();
        mediator.request_interactive("permission-web-1", &validated, scope.clone())?;
        let receipt = mediator.decide(
            "permission-web-1",
            &validated,
            &scope,
            LedgerPermissionDecision::Approved,
        )?;
        let changed = WebRequest::new(
            WebMethod::Head,
            CanonicalWebUrl::parse("https://example.com/other")?,
        );

        assert_eq!(
            authorize_web_request(
                "project-1",
                &validated,
                changed,
                receipt.authorization.ok_or("approval receipt missing")?,
            )
            .err(),
            Some(WebAuthorizationError::BindingMismatch)
        );
        Ok(())
    }

    #[test]
    fn denial_cancellation_stale_change_and_replay_never_reach_the_effect_leaf()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = registry()?;
        let validated = validated(
            &registry,
            "call-adversarial",
            "https://example.com/report?token=secret",
            "GET",
        )?;
        let approved = WebRequest::new(
            WebMethod::Get,
            CanonicalWebUrl::parse("https://example.com/report?token=secret")?,
        );
        let scope = web_permission_scope("project-1", &approved)?;
        let leaf_calls = AtomicUsize::new(0);

        for (permission_id, decision) in [
            ("permission-denied", LedgerPermissionDecision::Denied),
            ("permission-cancelled", LedgerPermissionDecision::Cancelled),
        ] {
            let mut mediator = PermissionMediator::default();
            mediator.request_interactive(permission_id, &validated, scope.clone())?;
            let receipt = mediator.decide(permission_id, &validated, &scope, decision)?;
            assert!(receipt.authorization.is_none());
            assert!(
                mediator
                    .decide(permission_id, &validated, &scope, decision)
                    .is_err()
            );
        }
        assert_eq!(leaf_calls.load(Ordering::Relaxed), 0);

        for changed in [
            WebRequest::new(
                WebMethod::Head,
                CanonicalWebUrl::parse("https://example.com/report?token=secret")?,
            ),
            WebRequest::new(
                WebMethod::Get,
                CanonicalWebUrl::parse("https://example.com/other")?,
            ),
        ] {
            let permission_id = format!("permission-change-{}", changed.method().as_str());
            let mut mediator = PermissionMediator::default();
            mediator.request_interactive(&permission_id, &validated, scope.clone())?;
            let receipt = mediator.decide(
                &permission_id,
                &validated,
                &scope,
                LedgerPermissionDecision::Approved,
            )?;
            if authorize_web_request(
                "project-1",
                &validated,
                changed,
                receipt.authorization.ok_or("approval missing")?,
            )
            .is_ok()
            {
                leaf_calls.fetch_add(1, Ordering::Relaxed);
            }
        }
        assert_eq!(leaf_calls.load(Ordering::Relaxed), 0);

        let mut mediator = PermissionMediator::default();
        mediator.request_interactive("permission-once", &validated, scope.clone())?;
        let receipt = mediator.decide(
            "permission-once",
            &validated,
            &scope,
            LedgerPermissionDecision::Approved,
        )?;
        let mut state = agent_core::AgentStateMachine::start("run-once", "generation-once")?;
        let requested = state.request_tool(&state.transition_token(), &validated)?;
        let awaiting = state.await_permission(&requested)?;
        authorize_web_request(
            "project-1",
            &validated,
            approved,
            receipt.authorization.ok_or("approval missing")?,
        )?
        .start(&mut state, &awaiting)?;
        leaf_calls.fetch_add(1, Ordering::Relaxed);
        assert!(
            mediator
                .decide(
                    "permission-once",
                    &validated,
                    &scope,
                    LedgerPermissionDecision::Approved,
                )
                .is_err()
        );
        assert_eq!(leaf_calls.load(Ordering::Relaxed), 1);
        Ok(())
    }
}
