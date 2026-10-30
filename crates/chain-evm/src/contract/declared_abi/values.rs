use super::super::{ContractError, abi::hex_bytes};
use alloy_dyn_abi::{DynSolType, DynSolValue};
use serde_json::Value;

pub(super) fn input(
    ty: &DynSolType,
    value: &Value,
    nodes: &mut usize,
) -> Result<DynSolValue, ContractError> {
    *nodes += 1;
    if *nodes > super::MAX_NODES {
        return Err(ContractError::InvalidArguments);
    }
    let fields = |types: &[DynSolType], values: &[Value], nodes: &mut usize| {
        if types.len() != values.len() {
            return Err(ContractError::InvalidArguments);
        }
        types
            .iter()
            .zip(values)
            .map(|(ty, value)| input(ty, value, nodes))
            .collect::<Result<Vec<_>, _>>()
    };
    match ty {
        DynSolType::Tuple(types) => Ok(DynSolValue::Tuple(fields(
            types,
            value.as_array().ok_or(ContractError::InvalidArguments)?,
            nodes,
        )?)),
        DynSolType::Array(inner) | DynSolType::FixedArray(inner, _) => {
            let values = value
                .as_array()
                .filter(|values| values.len() <= super::MAX_ITEMS)
                .ok_or(ContractError::InvalidArguments)?;
            if let DynSolType::FixedArray(_, size) = ty
                && values.len() != *size
            {
                return Err(ContractError::InvalidArguments);
            }
            let values = values
                .iter()
                .map(|value| input(inner, value, nodes))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(if matches!(ty, DynSolType::Array(_)) {
                DynSolValue::Array(values)
            } else {
                DynSolValue::FixedArray(values)
            })
        }
        DynSolType::Bool => match value {
            Value::Bool(boolean) => Ok(DynSolValue::Bool(*boolean)),
            Value::String(word) if word == "true" || word == "false" => {
                Ok(DynSolValue::Bool(word == "true"))
            }
            _ => Err(ContractError::InvalidArguments),
        },
        DynSolType::String => value
            .as_str()
            .filter(|value| value.len() <= super::MAX_BLOB)
            .map(|value| DynSolValue::String(value.into()))
            .ok_or(ContractError::InvalidArguments),
        DynSolType::Address => {
            let text = value.as_str().ok_or(ContractError::InvalidArguments)?;
            // Preserve the existing mixed-case EIP-55 check, recursively too.
            let address =
                crate::PublicEvmAddress::new(text).map_err(|_| ContractError::InvalidArguments)?;
            ty.coerce_str(address.as_str())
                .map_err(|_| ContractError::InvalidArguments)
        }
        _ => {
            let text = value
                .as_str()
                .filter(|text| text.len() <= super::MAX_BLOB * 2 + 2)
                .ok_or(ContractError::InvalidArguments)?;
            let result = ty
                .coerce_str(text)
                .map_err(|_| ContractError::InvalidArguments)?;
            // Disallow units, exponent notation, aliases and non-canonical numeric
            // strings accepted by a general convenience coercer.
            if output(&result) != Value::String(text.to_ascii_lowercase()) {
                return Err(ContractError::InvalidArguments);
            }
            Ok(result)
        }
    }
}
pub(super) fn output(value: &DynSolValue) -> Value {
    match value {
        DynSolValue::Bool(value) => Value::Bool(*value),
        DynSolValue::Int(value, _) => Value::String(value.to_string()),
        DynSolValue::Uint(value, _) => Value::String(value.to_string()),
        DynSolValue::Address(value) => Value::String(format!("{value:#x}")),
        DynSolValue::FixedBytes(value, size) => Value::String(hex_bytes(&value[..*size])),
        DynSolValue::Bytes(value) => Value::String(hex_bytes(value)),
        DynSolValue::String(value) => Value::String(value.clone()),
        DynSolValue::Tuple(values)
        | DynSolValue::Array(values)
        | DynSolValue::FixedArray(values) => Value::Array(values.iter().map(output).collect()),
        // Function pointers/custom structs are excluded by type admission.
        DynSolValue::Function(_) => Value::Null,
    }
}
