//! Exact compiler-ABI log interpretation. Event names prove no business result.
use super::{abi, error::GenericError, identity::GenericContractIdentity};
use crate::contract::DeclaredContractCall;
use alloy_primitives::{Address, keccak256};
use evm_simulation::LogObservation;
use serde::Serialize;
use serde_json::{Value, json};

/// Complete raw log with optional source-declared interpretation, never a balance proof.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericLogEffect {
    /// Exact emitting storage-context address.
    pub address: String,
    /// Complete raw ordered topics.
    pub topics: Vec<String>,
    /// Complete raw ABI body.
    pub data: String,
    /// Compiler-declared event signature.
    pub signature: String,
    /// Canonical values; dynamic indexed values remain explicit topic hashes.
    pub arguments: Vec<Value>,
}

pub(super) fn decode(
    identity: &GenericContractIdentity,
    logs: &[LogObservation],
) -> Result<Vec<GenericLogEffect>, GenericError> {
    if logs.len() > 64 || logs.iter().map(|log| log.data.len()).sum::<usize>() > 32 * 1024 {
        return Err(GenericError::Limit);
    }
    logs.iter()
        .map(|log| {
            let address: Address = log
                .address
                .parse()
                .map_err(|_| GenericError::SnapshotMismatch)?;
            let artifact = identity
                .storage_artifact(address)
                .ok_or(GenericError::SourceMismatch)?;
            decode_one(artifact.abi(), log)
        })
        .collect()
}
fn decode_one(abi: &Value, log: &LogObservation) -> Result<GenericLogEffect, GenericError> {
    let mut matches = Vec::new();
    for event in abi
        .as_array()
        .ok_or(GenericError::InvalidAbi)?
        .iter()
        .filter(|item| item["type"] == "event" && item["anonymous"] == false)
    {
        let signature = abi::signature(event)?;
        if log.topics.first() == Some(&format!("{:#x}", keccak256(signature.as_bytes()))) {
            matches.push((event, signature));
        }
    }
    if matches.len() != 1 {
        return Err(GenericError::InvalidAbi);
    }
    let (event, signature) = matches.pop().ok_or(GenericError::InvalidAbi)?;
    let types = abi::parameters(&event["inputs"])?;
    let inputs = event["inputs"].as_array().ok_or(GenericError::InvalidAbi)?;
    let body_types = inputs
        .iter()
        .zip(&types)
        .filter(|(input, _)| input["indexed"] == false)
        .map(|(_, ty)| ty.clone())
        .collect::<Vec<_>>();
    let decoder = DeclaredContractCall::new(&log.address, "decode()", &[], &body_types)
        .map_err(|_| GenericError::InvalidAbi)?;
    let body = decoder
        .decode_result(&log.data)
        .map_err(|_| GenericError::InvalidAbi)?;
    let mut body = body.into_iter();
    let mut topics = log.topics.iter().skip(1);
    let mut arguments = Vec::new();
    for (input, ty) in inputs.iter().zip(types) {
        let value = match input["indexed"].as_bool() {
            Some(false) => body.next().ok_or(GenericError::InvalidAbi)?,
            Some(true) => {
                let topic = topics.next().ok_or(GenericError::InvalidAbi)?;
                if topic.len() != 66
                    || !topic.starts_with("0x")
                    || !topic[2..].bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(GenericError::InvalidAbi);
                }
                if ty == "string" || ty == "bytes" || ty.contains('[') || ty.starts_with('(') {
                    json!({"indexedHash":topic})
                } else {
                    let decoder = DeclaredContractCall::new(&log.address, "decode()", &[], &[ty])
                        .map_err(|_| GenericError::InvalidAbi)?;
                    decoder
                        .decode_result(topic)
                        .map_err(|_| GenericError::InvalidAbi)?
                        .into_iter()
                        .next()
                        .ok_or(GenericError::InvalidAbi)?
                }
            }
            _ => return Err(GenericError::InvalidAbi),
        };
        arguments.push(value);
    }
    if body.next().is_some() || topics.next().is_some() {
        return Err(GenericError::InvalidAbi);
    }
    Ok(GenericLogEffect {
        address: log.address.clone(),
        topics: log.topics.clone(),
        data: log.data.clone(),
        signature,
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deceptive_transfer_name_never_establishes_token_effect_and_bad_shape_rejects()
    -> Result<(), GenericError> {
        let abi = json!([{"type":"event","name":"Transfer","anonymous":false,"inputs":[{"type":"uint256","indexed":false}]}]);
        let topic = format!("{:#x}", keccak256("Transfer(uint256)"));
        let log = LogObservation {
            address: "0x1111111111111111111111111111111111111111".into(),
            topics: vec![topic],
            data: format!("0x{:064x}", 7),
        };
        let decoded = decode_one(&abi, &log)?;
        assert_eq!(decoded.arguments, [json!("7")]);
        assert_eq!(decoded.signature, "Transfer(uint256)");
        let mut changed = log.clone();
        changed.topics.push(format!("0x{:064x}", 0));
        assert_eq!(decode_one(&abi, &changed), Err(GenericError::InvalidAbi));
        changed = log;
        changed.data.push_str("00");
        assert_eq!(decode_one(&abi, &changed), Err(GenericError::InvalidAbi));
        Ok(())
    }
}
