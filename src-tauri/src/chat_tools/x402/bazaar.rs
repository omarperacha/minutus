//! Bounded Bazaar discovery parsing and generic HTTP request materialization.

use std::collections::BTreeSet;

use super::transport::{Method, Scope};
use sha2::{Digest as _, Sha256};

use super::{hex, schema};

const RESOURCE_REFERENCE_PREFIX: &str = "ref:x402-resource-";
const MAX_DISCOVERED_RESOURCES: usize = 8;
const MAX_DISCOVERY_BYTES: usize = 64 * 1024;
const HOST_GENERATED_FIELDS: [&str; 2] = ["requestId", "accessToken"];

#[derive(Clone, Debug)]
pub(super) struct BazaarResource {
    reference: String,
    source_call_id: String,
    url: String,
    method: Method,
    body_example: serde_json::Value,
    model_input_schema: serde_json::Value,
    host_generated_fields: Vec<String>,
    output: Option<serde_json::Value>,
}

impl BazaarResource {
    pub(super) fn reference(&self) -> &str {
        &self.reference
    }

    pub(super) fn source_call_id(&self) -> &str {
        &self.source_call_id
    }

    pub(super) fn model_projection(&self) -> serde_json::Value {
        serde_json::json!({
            "resourceRef": self.reference,
            "resource": self.url,
            "method": self.method.as_str(),
            "bodyExample": strip_host_generated_fields(
                self.body_example.clone(),
                &self.host_generated_fields,
            ),
            "inputSchema": self.model_input_schema,
            "output": self.output,
            "hostGeneratedFields": self.host_generated_fields,
            "nextOperation": "purchase",
            "purchaseArguments": ["operation", "resourceRef", "input", "reason"],
            "live402Authoritative": true,
        })
    }

    pub(super) fn from_persisted(
        value: &serde_json::Value,
        source_call_id: &str,
        scope: &Scope,
    ) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "persisted resource is not an object".to_owned())?;
        let reference = string_field(object, "resourceRef")?;
        if !valid_resource_reference(reference) {
            return Err("persisted resource reference is invalid".to_owned());
        }
        let url = string_field(object, "resource")?;
        if !scope.admits(url) {
            return Err("persisted resource is outside the configured scope".to_owned());
        }
        if resource_reference(source_call_id, url) != reference {
            return Err("persisted resource reference does not match its source".to_owned());
        }
        let method = parse_method(string_field(object, "method")?)?;
        let body_example = object
            .get("bodyExample")
            .cloned()
            .ok_or_else(|| "persisted body example is missing".to_owned())?;
        let model_input_schema = object
            .get("inputSchema")
            .cloned()
            .ok_or_else(|| "persisted model input schema is missing".to_owned())?;
        schema::validate_schema(&model_input_schema)
            .map_err(|error| format!("{}: {}", error.path, error.message))?;
        let host_generated_fields = object
            .get("hostGeneratedFields")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "persisted host-generated field list is missing".to_owned())?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|field| HOST_GENERATED_FIELDS.contains(field))
                    .map(str::to_owned)
                    .ok_or_else(|| "persisted host-generated field is unsupported".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            reference: reference.to_owned(),
            source_call_id: source_call_id.to_owned(),
            url: url.to_owned(),
            method,
            body_example,
            model_input_schema,
            host_generated_fields,
            output: object
                .get("output")
                .cloned()
                .filter(|value| !value.is_null()),
        })
    }

    pub(super) fn materialize_body(
        &self,
        input: &serde_json::Value,
    ) -> Result<Vec<u8>, schema::SchemaError> {
        schema::validate_instance(&self.model_input_schema, input)?;
        let mut body = input.clone();
        let Some(fields) = body.as_object_mut() else {
            return Err(schema::SchemaError {
                path: "$.input".to_owned(),
                message: "resource input must be an object".to_owned(),
            });
        };
        if self
            .host_generated_fields
            .iter()
            .any(|field| field == "requestId")
        {
            fields.insert(
                "requestId".to_owned(),
                serde_json::Value::String(format!("zeta-{}", uuid::Uuid::now_v7())),
            );
        }
        if self
            .host_generated_fields
            .iter()
            .any(|field| field == "accessToken")
        {
            let mut secret = [0_u8; 32];
            getrandom::fill(&mut secret).map_err(|_| schema::SchemaError {
                path: "$.input.accessToken".to_owned(),
                message: "host could not generate the requested capability".to_owned(),
            })?;
            fields.insert(
                "accessToken".to_owned(),
                serde_json::Value::String(hex(&secret)),
            );
            secret.fill(0);
        }
        serde_json::to_vec(&body).map_err(|_| schema::SchemaError {
            path: "$.input".to_owned(),
            message: "resource input could not be encoded".to_owned(),
        })
    }

    pub(super) const fn method(&self) -> Method {
        self.method
    }

    pub(super) fn url(&self) -> &str {
        &self.url
    }
}

pub(super) fn parse_discovery_document(
    value: &serde_json::Value,
    call_id: &str,
    scope: &Scope,
) -> Result<Vec<BazaarResource>, String> {
    if serde_json::to_vec(value).map_or(true, |encoded| encoded.len() > MAX_DISCOVERY_BYTES) {
        return Err("Bazaar discovery document exceeds the 64 KiB limit".to_owned());
    }
    let root = value
        .as_object()
        .ok_or_else(|| "Bazaar discovery document must be an object".to_owned())?;
    if root.get("x402Version").and_then(serde_json::Value::as_u64) != Some(2) {
        return Err("Bazaar discovery document must declare x402Version 2".to_owned());
    }
    let items = root
        .get("items")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Bazaar discovery document must contain items".to_owned())?;
    if items.is_empty() || items.len() > MAX_DISCOVERED_RESOURCES {
        return Err("Bazaar discovery must contain 1 to 8 resources".to_owned());
    }
    let resources = items
        .iter()
        .map(|item| parse_resource(item, call_id, scope))
        .collect::<Result<Vec<_>, _>>()?;
    let references = resources
        .iter()
        .map(BazaarResource::reference)
        .collect::<BTreeSet<_>>();
    if references.len() != resources.len() {
        return Err("Bazaar discovery contains duplicate resource URLs".to_owned());
    }
    Ok(resources)
}

pub(super) fn valid_resource_reference(value: &str) -> bool {
    value
        .strip_prefix(RESOURCE_REFERENCE_PREFIX)
        .is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

fn parse_resource(
    value: &serde_json::Value,
    call_id: &str,
    scope: &Scope,
) -> Result<BazaarResource, String> {
    let item = value
        .as_object()
        .ok_or_else(|| "Bazaar resource must be an object".to_owned())?;
    if item.get("type").and_then(serde_json::Value::as_str) != Some("http")
        || item.get("x402Version").and_then(serde_json::Value::as_u64) != Some(2)
    {
        return Err("Bazaar resource must be an x402 v2 HTTP resource".to_owned());
    }
    let url = string_field(item, "resource")?;
    if !scope.admits(url) {
        return Err("Bazaar resource is outside the configured local scope".to_owned());
    }
    let accepts = item
        .get("accepts")
        .cloned()
        .ok_or_else(|| "Bazaar resource accepts are missing".to_owned())?;
    if accepts.as_array().is_none_or(Vec::is_empty) {
        return Err("Bazaar resource must advertise at least one payment option".to_owned());
    }
    let bazaar = value
        .pointer("/extensions/bazaar")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "Bazaar resource extension is missing".to_owned())?;
    let info = bazaar
        .get("info")
        .ok_or_else(|| "Bazaar resource info is missing".to_owned())?;
    let discovery_schema = bazaar
        .get("schema")
        .ok_or_else(|| "Bazaar resource schema is missing".to_owned())?;
    schema::validate_instance(discovery_schema, info)
        .map_err(|error| format!("Bazaar info {}: {}", error.path, error.message))?;
    let input = info
        .get("input")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "Bazaar HTTP input is missing".to_owned())?;
    if input.get("type").and_then(serde_json::Value::as_str) != Some("http")
        || input.get("bodyType").and_then(serde_json::Value::as_str) != Some("json")
    {
        return Err("Minutus currently supports Bazaar HTTP JSON body resources only".to_owned());
    }
    let method = parse_method(string_field(input, "method")?)?;
    let body_example = input
        .get("body")
        .cloned()
        .filter(serde_json::Value::is_object)
        .ok_or_else(|| "Bazaar JSON body example must be an object".to_owned())?;
    let body_schema = discovery_schema
        .pointer("/properties/input/properties/body")
        .cloned()
        .ok_or_else(|| "Bazaar schema must define input.body".to_owned())?;
    schema::validate_instance(&body_schema, &body_example)
        .map_err(|error| format!("Bazaar body example {}: {}", error.path, error.message))?;
    let host_generated_fields = HOST_GENERATED_FIELDS
        .into_iter()
        .filter(|field| {
            body_schema
                .pointer(&format!("/properties/{field}"))
                .is_some()
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    validate_host_field_contracts(&body_schema, &host_generated_fields)?;
    let model_input_schema = model_input_schema(&body_schema, &host_generated_fields)?;
    Ok(BazaarResource {
        reference: resource_reference(call_id, url),
        source_call_id: call_id.to_owned(),
        url: url.to_owned(),
        method,
        body_example,
        model_input_schema,
        host_generated_fields,
        output: info.get("output").cloned(),
    })
}

fn model_input_schema(
    body_schema: &serde_json::Value,
    host_generated_fields: &[String],
) -> Result<serde_json::Value, String> {
    let mut schema = body_schema.clone();
    let object = schema
        .as_object_mut()
        .ok_or_else(|| "Bazaar body schema must be an object".to_owned())?;
    if let Some(properties) = object
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        for field in host_generated_fields {
            properties.remove(field);
        }
    }
    if let Some(required) = object
        .get_mut("required")
        .and_then(serde_json::Value::as_array_mut)
    {
        required.retain(|field| {
            field
                .as_str()
                .is_none_or(|field| !host_generated_fields.iter().any(|host| host == field))
        });
    }
    schema::validate_schema(&schema).map_err(|error| {
        format!(
            "Bazaar model input schema {}: {}",
            error.path, error.message
        )
    })?;
    Ok(schema)
}

fn validate_host_field_contracts(
    body_schema: &serde_json::Value,
    fields: &[String],
) -> Result<(), String> {
    for field in fields {
        let field_schema = body_schema
            .pointer(&format!("/properties/{field}"))
            .ok_or_else(|| format!("Bazaar host field `{field}` has no schema"))?;
        let sample = match field.as_str() {
            "requestId" => serde_json::json!("zeta-00000000-0000-7000-8000-000000000000"),
            "accessToken" => serde_json::json!("0".repeat(64)),
            _ => return Err(format!("Bazaar host field `{field}` is unsupported")),
        };
        schema::validate_instance(field_schema, &sample).map_err(|error| {
            format!(
                "Bazaar host field `{field}` is incompatible at {}: {}",
                error.path, error.message
            )
        })?;
    }
    Ok(())
}

fn strip_host_generated_fields(
    mut value: serde_json::Value,
    host_generated_fields: &[String],
) -> serde_json::Value {
    if let Some(fields) = value.as_object_mut() {
        for field in host_generated_fields {
            fields.remove(field);
        }
    }
    value
}

fn parse_method(value: &str) -> Result<Method, String> {
    match value {
        "POST" => Ok(Method::Post),
        _ => Err("Minutus currently supports Bazaar POST resources only".to_owned()),
    }
}

fn string_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<&'a str, String> {
    object
        .get(name)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Bazaar field `{name}` must be a non-empty string"))
}

pub(super) fn resource_reference(call_id: &str, url: &str) -> String {
    let digest = Sha256::digest(format!("{call_id}\0{url}").as_bytes());
    format!("{RESOURCE_REFERENCE_PREFIX}{}", hex(&digest[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESOURCE_URL: &str = "https://example.com/widgets/purchase";

    fn generic_discovery() -> serde_json::Value {
        let body_schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["requestId", "accessToken", "widget"],
            "properties": {
                "requestId": {"type":"string","minLength":16,"maxLength":100},
                "accessToken": {"type":"string","pattern":"^[a-f0-9]{64}$"},
                "widget": {"type":"string","enum":["blue","green"]}
            }
        });
        serde_json::json!({
            "x402Version": 2,
            "items": [{
                "resource": RESOURCE_URL,
                "type": "http",
                "x402Version": 2,
                "accepts": [{"scheme":"exact","network":"eip155:84532","amount":"1"}],
                "extensions": {"bazaar": {
                    "info": {
                        "input": {
                            "type":"http",
                            "method":"POST",
                            "bodyType":"json",
                            "body": {
                                "requestId":"client-request-0001",
                                "accessToken":"0".repeat(64),
                                "widget":"blue"
                            }
                        },
                        "output":{"type":"json","example":{"purchased":true}}
                    },
                    "schema": {
                        "$schema":"https://json-schema.org/draft/2020-12/schema",
                        "type":"object",
                        "required":["input"],
                        "properties": {
                            "input": {
                                "type":"object",
                                "required":["type","method","bodyType","body"],
                                "properties": {
                                    "type":{"const":"http"},
                                    "method":{"const":"POST"},
                                    "bodyType":{"const":"json"},
                                    "body":body_schema
                                }
                            }
                        }
                    }
                }}
            }]
        })
    }

    #[test]
    fn generic_bazaar_resource_projects_schema_and_materializes_host_fields() -> Result<(), String>
    {
        let scope = Scope::Public;
        let resources = parse_discovery_document(&generic_discovery(), "discovery-call", &scope)?;
        let resource = resources.first().ok_or("resource")?;
        let projection = resource.model_projection();
        assert_eq!(
            projection["bodyExample"],
            serde_json::json!({"widget":"blue"})
        );
        assert_eq!(
            projection["hostGeneratedFields"],
            serde_json::json!(["requestId", "accessToken"])
        );
        let body = resource
            .materialize_body(&serde_json::json!({"widget":"green"}))
            .map_err(|error| error.message)?;
        let body: serde_json::Value =
            serde_json::from_slice(&body).map_err(|error| error.to_string())?;
        assert!(
            body["requestId"]
                .as_str()
                .is_some_and(|value| value.starts_with("zeta-"))
        );
        assert_eq!(body["accessToken"].as_str().map(str::len), Some(64));
        assert_eq!(body["widget"], "green");
        Ok(())
    }

    #[test]
    fn discovered_schema_rejects_invalid_model_input_without_payment() -> Result<(), String> {
        let scope = Scope::Public;
        let resources = parse_discovery_document(&generic_discovery(), "discovery-call", &scope)?;
        let Err(error) = resources[0].materialize_body(&serde_json::json!({"widget":"red"})) else {
            return Err("invalid enum was accepted".to_owned());
        };
        assert_eq!(error.path, "$.widget");
        Ok(())
    }
}
