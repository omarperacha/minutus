//! Payment extension admission, independent of discovery interpretation.

use serde_json::{Map, Value};

use super::X402PolicyError;

/// Bazaar is discovery metadata, not a signing mechanism. Retain its complete
/// envelope for the v2 echo contract; never interpret it as payment terms,
/// execute its examples, or resolve its schema references here. Parser bounds
/// apply to this data and preparation checks the complete outgoing header size.
pub(super) fn validate_payment_extensions(
    extensions: Option<&Map<String, Value>>,
) -> Result<(), X402PolicyError> {
    let Some(extensions) = extensions else {
        return Ok(());
    };
    if extensions.keys().any(|key| key != "bazaar") {
        return Err(X402PolicyError::UnsupportedExtension);
    }
    if let Some(bazaar) = extensions.get("bazaar") {
        let envelope = bazaar
            .as_object()
            .ok_or(X402PolicyError::InvalidExtensionData)?;
        if !envelope.get("info").is_some_and(Value::is_object)
            || !envelope.get("schema").is_some_and(Value::is_object)
        {
            return Err(X402PolicyError::InvalidExtensionData);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_bazaar_object_envelopes_with_info_and_schema_are_admitted() {
        for value in [json!({}), json!({"bazaar": {"info": {}, "schema": {}}})] {
            assert_eq!(validate_payment_extensions(value.as_object()), Ok(()));
        }
        for value in [
            json!({"bazaar": null}),
            json!({"bazaar": []}),
            json!({"bazaar": {"info": {}, "schema": false}}),
            json!({"bazaar": {"info": "invalid", "schema": {}}}),
            json!({"bazaar": {"info": {}}}),
            json!({"bazaar": {"schema": {}}}),
        ] {
            assert_eq!(
                validate_payment_extensions(value.as_object()),
                Err(X402PolicyError::InvalidExtensionData)
            );
        }
        for value in [
            json!({"payment-identifier": {"info": {}, "schema": {}}}),
            json!({"bazaar": {"info": {}, "schema": {}}, "unknown": {}}),
        ] {
            assert_eq!(
                validate_payment_extensions(value.as_object()),
                Err(X402PolicyError::UnsupportedExtension)
            );
        }
    }
}
