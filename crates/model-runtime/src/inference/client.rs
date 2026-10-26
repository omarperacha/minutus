//! Authenticated loopback HTTP client for the pinned `llama-server` API.

use std::{
    io::{BufRead, BufReader, Read},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header::{AUTHORIZATION, HeaderValue},
};
use serde::{Deserialize, Serialize};

use super::{GenerationRequest, InferenceError};
use crate::{
    GenerationCompletion, GenerationMetrics, SidecarEndpoint, blocking_client::BlockingClient,
    inference::types::GLIMMER_IMAGE_MARKER, sidecar::MEDIA_MARKER,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const TOKENIZE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONTROL_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SSE_LINE_BYTES: usize = 256 * 1024;

pub(super) struct ServerClient {
    streaming: BlockingClient,
    control: BlockingClient,
}

impl ServerClient {
    pub(super) fn new() -> Result<Self, InferenceError> {
        let streaming = BlockingClient::build(Client::builder().connect_timeout(CONNECT_TIMEOUT))
            .map_err(|()| InferenceError::RuntimeUnavailable)?;
        let control = BlockingClient::build(Client::builder().connect_timeout(CONNECT_TIMEOUT))
            .map_err(|()| InferenceError::RuntimeUnavailable)?;
        Ok(Self { streaming, control })
    }

    pub(super) fn health(
        &self,
        endpoint: &SidecarEndpoint,
    ) -> Result<HealthStatus, InferenceError> {
        let response = self
            .control
            .get(endpoint_url(endpoint, "/health"))
            .timeout(CONTROL_TIMEOUT)
            .send()
            .map_err(InferenceError::Server)?;
        match response.status() {
            StatusCode::OK => Ok(HealthStatus::Ready),
            StatusCode::SERVICE_UNAVAILABLE => Ok(HealthStatus::Loading),
            _ => Err(InferenceError::ServerRejected),
        }
    }

    pub(super) fn tokenize(
        &self,
        endpoint: &SidecarEndpoint,
        prompt: &str,
    ) -> Result<usize, InferenceError> {
        let response = self
            .control
            .post(endpoint_url(endpoint, "/tokenize"))
            .timeout(TOKENIZE_TIMEOUT)
            .header(AUTHORIZATION, authorization(endpoint)?)
            .json(&TokenizeRequest {
                content: prompt,
                add_special: false,
                parse_special: true,
            })
            .send()
            .map_err(InferenceError::Server)?;
        if !response.status().is_success() {
            return Err(InferenceError::ServerRejected);
        }
        let parsed: TokenizeResponse = read_bounded_json(response)?;
        Ok(parsed.tokens.len())
    }

    pub(super) fn stream(
        &self,
        endpoint: &SidecarEndpoint,
        request: &GenerationRequest,
        mut poll: impl FnMut() -> Result<(), InferenceError>,
        mut content: impl FnMut(&[u8]) -> Result<(), InferenceError>,
    ) -> Result<CompletionResult, InferenceError> {
        let encoded_images = request
            .images
            .iter()
            .map(|image| STANDARD.encode(image))
            .collect::<Vec<_>>();
        let multimodal_prompt = (!encoded_images.is_empty())
            .then(|| request.prompt.replace(GLIMMER_IMAGE_MARKER, MEDIA_MARKER));
        let prompt =
            multimodal_prompt
                .as_ref()
                .map_or(CompletionPrompt::Text(&request.prompt), |prompt| {
                    CompletionPrompt::Multimodal {
                        prompt_string: prompt,
                        multimodal_data: &encoded_images,
                    }
                });
        let response = self
            .streaming
            .post(endpoint_url(endpoint, "/completion"))
            .header(AUTHORIZATION, authorization(endpoint)?)
            .header("X-Conversation-Id", &request.id)
            .json(&CompletionRequest {
                prompt,
                stream: true,
                n_predict: request.output_tokens,
                id_slot: 0,
                cache_prompt: true,
                sse_ping_interval: 1,
            })
            .send()
            .map_err(InferenceError::Server)?;
        if !response.status().is_success() {
            return Err(InferenceError::ServerRejected);
        }
        parse_sse(response, &mut poll, &mut content)
    }

    pub(super) fn cancel(
        &self,
        endpoint: &SidecarEndpoint,
        generation_id: &str,
    ) -> Result<(), InferenceError> {
        let url = endpoint_url(endpoint, &format!("/v1/stream?conv_id={generation_id}"));
        let response = self
            .control
            .delete(url)
            .timeout(CONTROL_TIMEOUT)
            .header(AUTHORIZATION, authorization(endpoint)?)
            .send()
            .map_err(InferenceError::Server)?;
        if response.status() == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(InferenceError::ServerRejected)
        }
    }
}

pub(super) enum HealthStatus {
    Loading,
    Ready,
}

#[derive(Serialize)]
struct TokenizeRequest<'a> {
    content: &'a str,
    add_special: bool,
    parse_special: bool,
}

#[derive(Deserialize)]
struct TokenizeResponse {
    tokens: Vec<i64>,
}

#[derive(Serialize)]
struct CompletionRequest<'a> {
    prompt: CompletionPrompt<'a>,
    stream: bool,
    n_predict: u32,
    id_slot: u32,
    cache_prompt: bool,
    sse_ping_interval: u32,
}

#[derive(Serialize)]
#[serde(untagged)]
enum CompletionPrompt<'a> {
    Text(&'a str),
    Multimodal {
        prompt_string: &'a str,
        multimodal_data: &'a [String],
    },
}

#[derive(Deserialize)]
struct CompletionFrame {
    #[serde(default)]
    content: String,
    stop: bool,
    #[serde(default)]
    stop_type: String,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    tokens_cached: Option<u64>,
    #[serde(default)]
    tokens_evaluated: Option<u64>,
    #[serde(default)]
    timings: Option<CompletionTimings>,
}

#[derive(Deserialize)]
struct CompletionTimings {
    #[serde(default)]
    prompt_n: Option<u64>,
    #[serde(default)]
    prompt_ms: Option<f64>,
    #[serde(default)]
    predicted_n: Option<u64>,
    #[serde(default)]
    predicted_ms: Option<f64>,
}

pub(super) struct CompletionResult {
    pub(super) completion: GenerationCompletion,
    pub(super) metrics: Option<GenerationMetrics>,
}

fn parse_sse(
    response: impl Read,
    poll: &mut impl FnMut() -> Result<(), InferenceError>,
    content: &mut impl FnMut(&[u8]) -> Result<(), InferenceError>,
) -> Result<CompletionResult, InferenceError> {
    let mut reader = BufReader::new(response);
    let mut line = Vec::new();
    loop {
        poll()?;
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(InferenceError::StreamIo)?;
        if read == 0 {
            return Err(InferenceError::UnexpectedStreamEnd);
        }
        if line.len() > MAX_SSE_LINE_BYTES {
            return Err(InferenceError::StreamFrameTooLarge);
        }
        let line = std::str::from_utf8(&line).map_err(|_| InferenceError::MalformedStream)?;
        let line = line.trim_end_matches(['\r', '\n']);
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let frame: CompletionFrame =
            serde_json::from_str(data).map_err(|_| InferenceError::MalformedStream)?;
        if !frame.content.is_empty() {
            content(frame.content.as_bytes())?;
        }
        if frame.stop {
            if frame.truncated {
                return Err(InferenceError::ContextLimitExceeded);
            }
            let completion = if frame.stop_type == "limit" {
                GenerationCompletion::Length
            } else {
                GenerationCompletion::Stop
            };
            return Ok(CompletionResult {
                completion,
                metrics: completion_metrics(&frame),
            });
        }
    }
}

fn completion_metrics(frame: &CompletionFrame) -> Option<GenerationMetrics> {
    let timings = frame.timings.as_ref();
    let metrics = GenerationMetrics {
        prompt_tokens: frame.tokens_evaluated.or_else(|| {
            timings
                .and_then(|value| value.prompt_n)
                .and_then(|prompt| prompt.checked_add(frame.tokens_cached.unwrap_or_default()))
        }),
        cached_prompt_tokens: frame.tokens_cached,
        cache_write_prompt_tokens: None,
        generated_tokens: timings.and_then(|value| value.predicted_n),
        prompt_evaluation_ns: timings
            .and_then(|value| value.prompt_ms)
            .and_then(milliseconds_to_nanoseconds),
        generation_ns: timings
            .and_then(|value| value.predicted_ms)
            .and_then(milliseconds_to_nanoseconds),
        total_ns: None,
        load_ns: None,
        peak_memory_bytes: None,
    };
    (metrics != GenerationMetrics::default()).then_some(metrics)
}

fn milliseconds_to_nanoseconds(value: f64) -> Option<u64> {
    let duration = Duration::try_from_secs_f64(value / 1_000.0).ok()?;
    u64::try_from(duration.as_nanos()).ok()
}

fn read_bounded_json<T: for<'de> Deserialize<'de>>(
    response: Response,
) -> Result<T, InferenceError> {
    let mut bytes = Vec::new();
    response
        .take(MAX_CONTROL_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(InferenceError::StreamIo)?;
    if u64::try_from(bytes.len()).map_or(true, |length| length > MAX_CONTROL_RESPONSE_BYTES) {
        return Err(InferenceError::ControlResponseTooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| InferenceError::MalformedServerResponse)
}

fn authorization(endpoint: &SidecarEndpoint) -> Result<HeaderValue, InferenceError> {
    HeaderValue::from_str(&format!("Bearer {}", endpoint.credential().expose_hex()))
        .map_err(|_| InferenceError::InvalidCredential)
}

fn endpoint_url(endpoint: &SidecarEndpoint, path: &str) -> String {
    format!("http://{}{path}", endpoint.address())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn multimodal_completion_uses_inline_data_and_fixed_marker()
    -> Result<(), Box<dyn std::error::Error>> {
        let images = vec![STANDARD.encode(b"verified png")];
        let prompt = format!("describe {MEDIA_MARKER}");
        let request = CompletionRequest {
            prompt: CompletionPrompt::Multimodal {
                prompt_string: &prompt,
                multimodal_data: &images,
            },
            stream: true,
            n_predict: 32,
            id_slot: 0,
            cache_prompt: true,
            sse_ping_interval: 1,
        };
        let json = serde_json::to_value(request)?;
        assert_eq!(json["prompt"]["prompt_string"], prompt);
        assert_eq!(json["prompt"]["multimodal_data"][0], images[0]);
        assert_eq!(json["id_slot"], 0);
        assert_eq!(json["cache_prompt"], true);
        assert!(!json.to_string().contains("http:"));
        assert!(!json.to_string().contains("https:"));
        assert!(!json.to_string().contains("file:"));
        Ok(())
    }

    #[test]
    fn parses_stream_content_and_length_completion() -> Result<(), Box<dyn std::error::Error>> {
        let input = concat!(
            ": ping\n\n",
            "data: {\"content\":\"hello\",\"stop\":false}\n\n",
            "data: {\"content\":\"\",\"stop\":true,\"stop_type\":\"limit\",\"truncated\":false,\"tokens_cached\":80,\"tokens_evaluated\":120,\"timings\":{\"prompt_n\":40,\"prompt_ms\":12.5,\"predicted_n\":30,\"predicted_ms\":25.25}}\n\n"
        );
        let mut output = Vec::new();
        let completion = parse_sse(Cursor::new(input), &mut || Ok(()), &mut |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        })?;
        assert_eq!(output, b"hello");
        assert_eq!(completion.completion, GenerationCompletion::Length);
        assert_eq!(
            completion.metrics,
            Some(GenerationMetrics {
                prompt_tokens: Some(120),
                cached_prompt_tokens: Some(80),
                cache_write_prompt_tokens: None,
                generated_tokens: Some(30),
                prompt_evaluation_ns: Some(12_500_000),
                generation_ns: Some(25_250_000),
                total_ns: None,
                load_ns: None,
                peak_memory_bytes: None,
            })
        );
        Ok(())
    }

    #[test]
    fn rejects_truncated_context_and_unterminated_stream() {
        let truncated =
            "data: {\"content\":\"\",\"stop\":true,\"stop_type\":\"eos\",\"truncated\":true}\n";
        assert!(matches!(
            parse_sse(Cursor::new(truncated), &mut || Ok(()), &mut |_| Ok(())),
            Err(InferenceError::ContextLimitExceeded)
        ));
        let partial = "data: {\"content\":\"hello\",\"stop\":false}\n";
        assert!(matches!(
            parse_sse(Cursor::new(partial), &mut || Ok(()), &mut |_| Ok(())),
            Err(InferenceError::UnexpectedStreamEnd)
        ));
    }
}
