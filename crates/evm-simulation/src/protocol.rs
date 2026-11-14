use crate::contract::{
    MAX_FRAME, SimulationError, SimulationObservation, SimulationRequest, StateQuery, StateValue,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{Read, Write};

#[derive(Deserialize, Serialize)]
#[serde(tag = "message", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum WorkerMessage {
    State {
        sequence: u32,
        query: StateQuery,
    },
    Finished {
        result: Result<SimulationObservation, SimulationError>,
    },
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "message", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum HostMessage {
    Start {
        version: u8,
        request: Box<SimulationRequest>,
    },
    State {
        sequence: u32,
        value: StateValue,
    },
}

pub(crate) fn read_frame<T: DeserializeOwned>(input: &mut impl Read) -> Result<T, SimulationError> {
    let mut length = [0u8; 4];
    input
        .read_exact(&mut length)
        .map_err(|_| SimulationError::Protocol)?;
    let length = usize::try_from(u32::from_be_bytes(length)).map_err(|_| SimulationError::Limit)?;
    if length > MAX_FRAME || length == 0 {
        return Err(SimulationError::Limit);
    }
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .map_err(|_| SimulationError::Protocol)?;
    serde_json::from_slice(&bytes).map_err(|_| SimulationError::Protocol)
}
// EOF is distinct from truncated/malformed trailing frames. Shared supervisors
// require clean EOF after the single terminal response.
pub(crate) fn read_next_frame<T: DeserializeOwned>(
    input: &mut impl Read,
) -> Result<Option<T>, SimulationError> {
    let mut first = [0u8; 1];
    match input.read(&mut first) {
        Ok(0) => Ok(None),
        Ok(_) => read_frame(&mut first.as_slice().chain(input)).map(Some),
        Err(_) => Err(SimulationError::Protocol),
    }
}
pub(crate) fn write_frame(
    value: &impl Serialize,
    output: &mut impl Write,
) -> Result<usize, SimulationError> {
    let bytes = serde_json::to_vec(value).map_err(|_| SimulationError::Protocol)?;
    if bytes.len() > MAX_FRAME {
        return Err(SimulationError::Limit);
    }
    output
        .write_all(
            &u32::try_from(bytes.len())
                .map_err(|_| SimulationError::Limit)?
                .to_be_bytes(),
        )
        .and_then(|()| output.write_all(&bytes))
        .and_then(|()| output.flush())
        .map_err(|_| SimulationError::Protocol)?;
    Ok(bytes.len())
}

pub(crate) struct StatePipe<R, W> {
    input: R,
    output: W,
    sequence: u32,
    total_bytes: usize,
}
impl<R: Read, W: Write> StatePipe<R, W> {
    pub(crate) fn new(input: R, output: W) -> Self {
        Self {
            input,
            output,
            sequence: 0,
            total_bytes: 0,
        }
    }
    pub(crate) fn query(&mut self, query: StateQuery) -> Result<StateValue, SimulationError> {
        self.sequence += 1;
        if self.sequence > crate::MAX_STATE_QUERIES {
            return Err(SimulationError::Limit);
        }
        write_frame(
            &WorkerMessage::State {
                sequence: self.sequence,
                query,
            },
            &mut self.output,
        )?;
        let HostMessage::State { sequence, value } = read_frame(&mut self.input)? else {
            return Err(SimulationError::Protocol);
        };
        if sequence != self.sequence {
            return Err(SimulationError::Protocol);
        }
        self.total_bytes += serde_json::to_vec(&value)
            .map_err(|_| SimulationError::Protocol)?
            .len();
        if self.total_bytes > crate::MAX_STATE_BYTES {
            return Err(SimulationError::Limit);
        }
        Ok(value)
    }
}

pub(crate) fn commitment(request: &SimulationRequest) -> Result<String, SimulationError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(request).map_err(|_| SimulationError::Protocol)?;
    if bytes.len() > MAX_FRAME {
        return Err(SimulationError::Limit);
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_public_request_round_trips_without_numeric_coercion()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = crate::SimulationRequest {
            network: crate::SimulationNetwork::EthereumMainnet,
            block: crate::SimulationBlock {
                hash: format!("0x{}", "11".repeat(32)),
                state_root: format!("0x{}", "22".repeat(32)),
                number: 26_000_000,
                timestamp: 1_793_300_000,
                beneficiary: format!("0x{}", "33".repeat(20)),
                gas_limit: 30_000_000,
                base_fee: 1,
                prevrandao: format!("0x{}", "44".repeat(32)),
                excess_blob_gas: 0,
                slot_number: None,
                block_access_list_hash: None,
            },
            transaction: crate::SimulationTransaction {
                caller: format!("0x{}", "11".repeat(20)),
                target: format!("0x{}", "22".repeat(20)),
                value: "0x0".into(),
                calldata: "0x".into(),
                nonce: 3,
                gas_limit: 1_000_000,
                max_fee_per_gas: u128::MAX,
                priority_fee_per_gas: u128::MAX - 1,
            },
        };
        let value = HostMessage::Start {
            version: 1,
            request: Box::new(request),
        };
        let encoded = serde_json::to_vec(&value)?;
        let decoded: HostMessage = serde_json::from_slice(&encoded)?;
        assert_eq!(serde_json::to_vec(&decoded)?, encoded);
        let original: serde_json::Value = serde_json::from_slice(&encoded)?;
        for invalid in [
            serde_json::json!(1),
            serde_json::json!("01"),
            serde_json::json!("-1"),
            serde_json::json!("340282366920938463463374607431768211456"),
        ] {
            let mut changed = original.clone();
            changed["request"]["transaction"]["maxFeePerGas"] = invalid;
            assert!(serde_json::from_value::<HostMessage>(changed).is_err());
        }
        Ok(())
    }

    #[test]
    fn invalid_frames_and_state_sequences_are_terminal() -> Result<(), SimulationError> {
        for length in [0u32, 65_537, u32::MAX] {
            assert!(matches!(
                read_frame::<HostMessage>(&mut &length.to_be_bytes()[..]),
                Err(SimulationError::Limit)
            ));
        }
        assert!(read_frame::<HostMessage>(&mut &b"\0\0\0\x10{}"[..]).is_err());
        let mut bytes = Vec::new();
        write_frame(
            &serde_json::json!({"message":"state","sequence":1,"value":{"kind":"storage","value":"0x0"},"endpoint":"http://bad"}),
            &mut bytes,
        )?;
        assert!(read_frame::<HostMessage>(&mut bytes.as_slice()).is_err());
        for sequence in [0, 2] {
            let mut bytes = Vec::new();
            write_frame(
                &HostMessage::State {
                    sequence,
                    value: StateValue::Storage {
                        value: "0x0".into(),
                    },
                },
                &mut bytes,
            )?;
            let mut output = Vec::new();
            let mut pipe = StatePipe::new(bytes.as_slice(), &mut output);
            assert!(matches!(
                pipe.query(StateQuery::BlockHash { number: 1 }),
                Err(SimulationError::Protocol)
            ));
        }
        Ok(())
    }
}
