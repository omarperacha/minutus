use std::io::{Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{ComputationRequest, ComputationResponse, RunnerError, RunnerStatus, engine};

pub(crate) const PROTOCOL_VERSION: u16 = 1;
pub(crate) const LAUNCH_TOKEN_ENV: &str = "ZETA_COMPUTATION_LAUNCH_TOKEN";
pub(crate) const LAUNCH_TOKEN_BYTES: usize = 32;
const MAX_REQUEST_FRAME_BYTES: usize = 12 * 1024 * 1024;
const MAX_RESPONSE_FRAME_BYTES: usize = 512 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RequestEnvelope {
    protocol_version: u16,
    launch_token: String,
    request: ComputationRequest,
}

impl RequestEnvelope {
    pub(crate) fn new(launch_token: String, request: ComputationRequest) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            launch_token,
            request,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResponseEnvelope {
    protocol_version: u16,
    launch_token: String,
    response: ComputationResponse,
}

pub(crate) fn run_one_shot(
    mut input: impl Read,
    mut output: impl Write,
) -> Result<(), RunnerError> {
    let expected_token = std::env::var(LAUNCH_TOKEN_ENV).map_err(|_| RunnerError::Unavailable)?;
    if !valid_token(&expected_token) {
        return Err(RunnerError::Unavailable);
    }
    let envelope: RequestEnvelope = read_frame(&mut input, MAX_REQUEST_FRAME_BYTES)?;
    if envelope.protocol_version != PROTOCOL_VERSION
        || !constant_time_equal(envelope.launch_token.as_bytes(), expected_token.as_bytes())
        || envelope.request.validate().is_err()
    {
        return Err(RunnerError::InvalidProtocol);
    }
    let response = if crate::resource_policy::apply_process_limits().is_ok() {
        engine::execute(&envelope.request)
    } else {
        ComputationResponse::terminal(
            envelope.request.request_id(),
            RunnerStatus::PolicyUnavailable,
            None,
        )
    };
    write_frame(
        &mut output,
        &ResponseEnvelope {
            protocol_version: PROTOCOL_VERSION,
            launch_token: expected_token,
            response,
        },
        MAX_RESPONSE_FRAME_BYTES,
    )
}

pub(crate) fn encode_request(
    envelope: &RequestEnvelope,
    mut output: impl Write,
) -> Result<(), RunnerError> {
    write_frame(&mut output, envelope, MAX_REQUEST_FRAME_BYTES)
}

pub(crate) fn decode_response(
    mut input: impl Read,
    expected_token: &str,
) -> Result<ComputationResponse, RunnerError> {
    let envelope: ResponseEnvelope = read_frame(&mut input, MAX_RESPONSE_FRAME_BYTES)?;
    if envelope.protocol_version != PROTOCOL_VERSION
        || !constant_time_equal(envelope.launch_token.as_bytes(), expected_token.as_bytes())
    {
        return Err(RunnerError::InvalidProtocol);
    }
    Ok(envelope.response)
}

fn read_frame<T: DeserializeOwned>(input: &mut impl Read, limit: usize) -> Result<T, RunnerError> {
    let mut length = [0_u8; 4];
    input
        .read_exact(&mut length)
        .map_err(|_| RunnerError::InvalidProtocol)?;
    let length =
        usize::try_from(u32::from_le_bytes(length)).map_err(|_| RunnerError::InvalidProtocol)?;
    if length == 0 || length > limit {
        return Err(RunnerError::InvalidProtocol);
    }
    let mut payload = vec![0_u8; length];
    input
        .read_exact(&mut payload)
        .map_err(|_| RunnerError::InvalidProtocol)?;
    serde_json::from_slice(&payload).map_err(|_| RunnerError::InvalidProtocol)
}

fn write_frame<T: Serialize>(
    output: &mut impl Write,
    value: &T,
    limit: usize,
) -> Result<(), RunnerError> {
    let payload = serde_json::to_vec(value).map_err(|_| RunnerError::InvalidProtocol)?;
    if payload.is_empty() || payload.len() > limit || payload.len() > u32::MAX as usize {
        return Err(RunnerError::InvalidProtocol);
    }
    let length = u32::try_from(payload.len()).map_err(|_| RunnerError::InvalidProtocol)?;
    output
        .write_all(&length.to_le_bytes())
        .and_then(|()| output.write_all(&payload))
        .and_then(|()| output.flush())
        .map_err(|_| RunnerError::Unavailable)
}

pub(crate) fn valid_token(value: &str) -> bool {
    value.len() == LAUNCH_TOKEN_BYTES * 2 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}
