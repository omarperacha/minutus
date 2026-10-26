use super::{BTreeMap, IpcError, IpcErrorCode};

pub(super) fn bounded_object_string(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    minimum: usize,
    maximum: usize,
    index: usize,
) -> Result<String, IpcError> {
    object
        .get(field)
        .and_then(|value| bounded_string(value, minimum, maximum))
        .ok_or_else(|| {
            contract_error(&format!(
                "Plan step at steps[{index}] has an invalid or missing `{field}`"
            ))
        })
}

pub(super) fn bounded_object_string_array(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    minimum: usize,
    maximum: usize,
    step_id: &str,
) -> Result<Vec<String>, IpcError> {
    object
        .get(field)
        .and_then(serde_json::Value::as_array)
        .filter(|values| (minimum..=maximum).contains(&values.len()))
        .ok_or_else(|| {
            contract_error(&format!(
                "Plan step `{step_id}` must encode `{field}` as a bounded array"
            ))
        })?
        .iter()
        .map(|value| {
            bounded_string(value, 1, 32).ok_or_else(|| {
                contract_error(&format!(
                    "Plan step `{step_id}` `{field}` entries must be bounded identifiers"
                ))
            })
        })
        .collect()
}

pub(super) fn bounded_string_array(
    arguments: &BTreeMap<String, serde_json::Value>,
    field: &str,
    minimum: usize,
    maximum: usize,
) -> Result<Vec<String>, IpcError> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_array)
        .filter(|values| (minimum..=maximum).contains(&values.len()))
        .ok_or_else(|| contract_error(&format!("`{field}` must be a bounded string array")))?
        .iter()
        .map(|value| {
            bounded_string(value, 1, 256)
                .ok_or_else(|| contract_error(&format!("`{field}` entries are invalid")))
        })
        .collect()
}

pub(super) fn bounded_string(
    value: &serde_json::Value,
    minimum: usize,
    maximum: usize,
) -> Option<String> {
    value
        .as_str()
        .filter(|value| (minimum..=maximum).contains(&value.len()))
        .map(str::to_owned)
}

pub(super) fn contract_error(message: &str) -> IpcError {
    IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: message.to_owned(),
        retryable: true,
    }
}
