//! Metadata-only startup surface; payment operations still require plan admission.

use super::{
    DEVELOPMENT_SCOPE_DESCRIPTION, MAX_REASON_BYTES, Operation, ParsedToolCall,
    PromptToolDefinition, operation,
};

pub(crate) fn discovery_tool_definition() -> PromptToolDefinition {
    PromptToolDefinition {
        name: "x402.request".to_owned(),
        description: format!(
            "{DEVELOPMENT_SCOPE_DESCRIPTION} For a documentation page, use web.retrieve first and follow its published JSON discovery link. Do not pass Markdown/HTML documentation URLs to discover or interpret a failed attempt as missing app support. When asked to discover an endpoint's requirements, call discover before asking for purchase details. Discovery needs only the supplied URL and a reason, not item choices, contact details or payment approval. Returns resourceRef, inputSchema, bodyExample and hostGeneratedFields. Ground questions for missing values in the returned inputSchema; bundled product reference material is not the endpoint's schema. Before a purchase, submit a plan to load the purchase schema; native input review and payment approval remain required."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {"type": "string", "enum": ["discover"]},
                "url": {"type": "string", "description": "Exact supplied Bazaar discovery URL."},
                "reason": {"type": "string", "minLength": 1, "maxLength": MAX_REASON_BYTES}
            },
            "required": ["operation", "url", "reason"],
            "additionalProperties": false
        }),
    }
}

pub(crate) fn is_discovery_call(call: &ParsedToolCall) -> bool {
    call.name == "x402.request" && matches!(operation(call), Ok(Operation::Discover))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_exposes_only_metadata_and_never_admits_payment_variants() {
        let tool = discovery_tool_definition();
        assert_eq!(
            tool.parameters["properties"]["operation"]["enum"],
            serde_json::json!(["discover"])
        );
        assert_eq!(tool.parameters["additionalProperties"], false);
        assert!(tool.parameters["properties"].get("input").is_none());
        assert!(tool.parameters["properties"].get("resourceRef").is_none());
        for (value, expected) in [
            ("discover", true),
            ("purchase", false),
            ("purchase_url", false),
            ("unknown", false),
        ] {
            let call = ParsedToolCall {
                call_id: "call".into(),
                recipient: tool.name.clone(),
                name: tool.name.clone(),
                arguments: std::collections::BTreeMap::from([(
                    "operation".into(),
                    serde_json::json!(value),
                )]),
            };
            assert_eq!(is_discovery_call(&call), expected);
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn metadata_still_requires_scoped_valid_arguments() -> Result<(), Box<dyn std::error::Error>> {
        let scope = super::super::transport::Scope::parse(
            "http://127.0.0.1:5001/test-project/us-central1/api/x402",
        )
        .map_err(|_| "scope")?;
        let mut call = ParsedToolCall {
            call_id: "metadata".into(),
            recipient: "x402.request".into(),
            name: "x402.request".into(),
            arguments: serde_json::from_value(
                serde_json::json!({"operation":"discover", "url":"http://127.0.0.1:5001/test-project/us-central1/api/x402/resources", "reason":"Read requirements"}),
            )?,
        };
        assert!(super::super::validate_call_in_scope(&call, &scope).is_ok());
        call.arguments
            .insert("input".into(), serde_json::json!({"unexpected":"body"}));
        assert!(super::super::validate_call_in_scope(&call, &scope).is_err());
        call.arguments.remove("input");
        call.arguments
            .insert("url".into(), serde_json::json!("https://example.com/other"));
        assert!(super::super::validate_call_in_scope(&call, &scope).is_err());
        Ok(())
    }
}
