use super::{GenericContractIdentity, GenericError, layout};
use crate::contract::DeclaredContractCall;
use alloy_primitives::Address;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Admits a future source-field condition's path, key/index syntax and exact
/// expected type before any write. It does not assert the condition holds or
/// require a future dynamic-array element to exist at today's snapshot.
///
/// # Errors
/// Rejects unresolved source/layout, opaque paths, invalid canonical expected
/// values, unsupported ordered types and fixed resource bounds.
pub fn admit_generic_condition(
    identity: &GenericContractIdentity,
    condition: &GenericCondition,
) -> Result<(), GenericError> {
    generic_condition_type(identity, condition).map(|_| ())
}

/// Returns the admitted primitive/bytes/string Solidity type for conservative
/// report sizing. This is interpretation data and grants no write authority.
///
/// # Errors
/// Applies the same source/path/key/value/type admission as the condition gate.
pub fn generic_condition_type(
    identity: &GenericContractIdentity,
    condition: &GenericCondition,
) -> Result<String, GenericError> {
    let address: Address = condition
        .target
        .parse()
        .map_err(|_| GenericError::InvalidAbi)?;
    let artifact = identity
        .storage_artifact(address)
        .ok_or(GenericError::SourceMismatch)?;
    let ty = layout::field_type(artifact.storage_layout(), &condition.path)?;
    validate_expected(&ty, condition.comparison, &condition.expected)?;
    Ok(ty)
}

pub(super) fn validate_expected(
    ty: &str,
    comparison: GenericComparison,
    expected: &Value,
) -> Result<(), GenericError> {
    if comparison != GenericComparison::Equal && !(ty.starts_with("uint") || ty.starts_with("int"))
    {
        return Err(GenericError::InvalidAbi);
    }
    let call = DeclaredContractCall::new(
        "0x1111111111111111111111111111111111111111",
        &format!("check({ty})"),
        std::slice::from_ref(expected),
        &[],
    )
    .map_err(|_| GenericError::InvalidAbi)?;
    if call.arguments() != std::slice::from_ref(expected) {
        return Err(GenericError::InvalidAbi);
    }
    Ok(())
}

/// An explicit source-field predicate, never an assertion of token semantics.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenericCondition {
    /// Exact storage owner, not an implementation or friendly label.
    pub target: String,
    /// Source variable/member names, canonical mapping keys and array indices.
    pub path: Vec<String>,
    /// Typed comparison; ordering is admitted only for integer fields.
    pub comparison: GenericComparison,
    /// Canonical typed expected value. Integers remain decimal strings.
    pub expected: Value,
}

/// Deliberately bounded source-field comparison vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GenericComparison {
    /// Exact canonical typed equality.
    Equal,
    /// Integer value greater than or equal to the expected value.
    AtLeast,
    /// Integer value less than or equal to the expected value.
    AtMost,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn future_conditions_require_canonical_type_ranges_and_integer_ordering()
    -> Result<(), GenericError> {
        validate_expected("uint8", GenericComparison::AtLeast, &json!("255"))?;
        validate_expected("int8", GenericComparison::AtMost, &json!("-128"))?;
        validate_expected("bool", GenericComparison::Equal, &json!(true))?;
        for (ty, comparison, value) in [
            ("uint8", GenericComparison::Equal, json!("256")),
            ("int8", GenericComparison::Equal, json!("-129")),
            ("uint256", GenericComparison::Equal, json!("01")),
            ("bool", GenericComparison::Equal, json!("true")),
            (
                "address",
                GenericComparison::AtLeast,
                json!("0x1111111111111111111111111111111111111111"),
            ),
        ] {
            assert_eq!(
                validate_expected(ty, comparison, &value),
                Err(GenericError::InvalidAbi)
            );
        }
        Ok(())
    }
}
