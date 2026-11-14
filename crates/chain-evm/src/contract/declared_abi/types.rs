use super::super::ContractError;
use alloy_dyn_abi::DynSolType;

pub(super) fn parse(value: &str) -> Result<DynSolType, ContractError> {
    // Bound syntax before the recursive upstream parser sees it.
    if value.len() > 512 || !value.is_ascii() {
        return Err(ContractError::UnsupportedAbi);
    }
    let mut depth = 0u8;
    for byte in value.bytes() {
        match byte {
            b'(' => {
                depth += 1;
                if depth > 5 {
                    return Err(ContractError::UnsupportedAbi);
                }
            }
            b')' => depth = depth.checked_sub(1).ok_or(ContractError::UnsupportedAbi)?,
            b'a'..=b'z' | b'0'..=b'9' | b'[' | b']' | b',' => {}
            _ => return Err(ContractError::UnsupportedAbi),
        }
    }
    if depth != 0 {
        return Err(ContractError::UnsupportedAbi);
    }
    let ty = DynSolType::parse(value).map_err(|_| ContractError::UnsupportedAbi)?;
    // Alloy renders singleton tuples with a Rust-style trailing comma. Solidity
    // canonical function/type spelling omits that comma, including nested ones.
    if ty.sol_type_name().replace(",)", ")") != value {
        return Err(ContractError::UnsupportedAbi);
    }
    validate(&ty, 0, &mut 0)?;
    Ok(ty)
}

fn validate(ty: &DynSolType, depth: usize, nodes: &mut usize) -> Result<(), ContractError> {
    *nodes += 1;
    if depth > 4 || *nodes > super::MAX_NODES {
        return Err(ContractError::UnsupportedAbi);
    }
    match ty {
        DynSolType::Uint(bits) | DynSolType::Int(bits)
            if (8..=256).contains(bits) && bits % 8 == 0 => {}
        DynSolType::FixedBytes(size) if (1..=32).contains(size) => {}
        DynSolType::Bool | DynSolType::Address | DynSolType::Bytes | DynSolType::String => {}
        DynSolType::Array(inner) => validate(inner, depth + 1, nodes)?,
        DynSolType::FixedArray(inner, size) if (1..=super::MAX_ITEMS).contains(size) => {
            validate(inner, depth + 1, nodes)?;
        }
        DynSolType::Tuple(fields) if fields.len() <= 16 && (!fields.is_empty() || depth == 0) => {
            for field in fields {
                validate(field, depth + 1, nodes)?;
            }
        }
        _ => return Err(ContractError::UnsupportedAbi),
    }
    Ok(())
}
