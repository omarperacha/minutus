//! Incremental Ollama NDJSON decoding for Qwen output.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    GenerationCompletion, GenerationEvent, GenerationMetrics, ModelOutputDecoder,
    ModelOutputFailureKind, ParsedToolCall,
};

const MAX_GENERATION_ID_BYTES: usize = 128;
const MAX_STREAM_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_DECODED_BYTES: usize = 16 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_DELTA_BYTES: usize = 32 * 1024;
const MAX_TOOL_CALLS: usize = 64;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 1024 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const THINK_DELIMITERS: [&str; 2] = ["<think>", "</think>"];

#[derive(Default)]
struct ReasoningDelimiterFilter {
    pending: String,
}

impl ReasoningDelimiterFilter {
    fn push(&mut self, text: &str) -> String {
        self.pending.push_str(text);
        let mut filtered = String::new();
        loop {
            let next = THINK_DELIMITERS
                .iter()
                .filter_map(|delimiter| {
                    self.pending
                        .find(delimiter)
                        .map(|index| (index, delimiter.len()))
                })
                .min_by_key(|(index, _)| *index);
            let Some((index, length)) = next else {
                break;
            };
            filtered.push_str(&self.pending[..index]);
            self.pending.drain(..index + length);
        }

        let retained = THINK_DELIMITERS
            .iter()
            .filter_map(|delimiter| {
                (1..delimiter.len()).rev().find(|length| {
                    *length <= self.pending.len() && self.pending.ends_with(&delimiter[..*length])
                })
            })
            .max()
            .unwrap_or(0);
        let emitted = self.pending.len().saturating_sub(retained);
        filtered.push_str(&self.pending[..emitted]);
        self.pending.drain(..emitted);
        filtered
    }

    fn finish(&mut self) -> String {
        std::mem::take(&mut self.pending)
    }
}

/// Stateful Qwen decoder for arbitrary chunks of Ollama NDJSON.
pub struct QwenStreamDecoder {
    generation_id: String,
    buffer: Vec<u8>,
    decoded_bytes: usize,
    output_bytes: usize,
    next_call_ordinal: u32,
    pending_calls: Vec<ParsedToolCall>,
    pending_content: String,
    reasoning_delimiters: ReasoningDelimiterFilter,
    content_delimiters: ReasoningDelimiterFilter,
    seen_call_ids: BTreeSet<String>,
    seen_call_fingerprints: BTreeSet<Vec<u8>>,
    terminal: bool,
}

impl QwenStreamDecoder {
    /// Creates a decoder with request-scoped deterministic fallback call IDs.
    ///
    /// # Errors
    ///
    /// Rejects an empty, excessive, or unsafe generation identifier.
    pub fn new(generation_id: impl Into<String>) -> Result<Self, QwenStreamError> {
        let generation_id = generation_id.into();
        if generation_id.is_empty()
            || generation_id.len() > MAX_GENERATION_ID_BYTES
            || !generation_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
        {
            return Err(QwenStreamError::InvalidGenerationId);
        }
        Ok(Self {
            generation_id,
            buffer: Vec::new(),
            decoded_bytes: 0,
            output_bytes: 0,
            next_call_ordinal: 0,
            pending_calls: Vec::new(),
            pending_content: String::new(),
            reasoning_delimiters: ReasoningDelimiterFilter::default(),
            content_delimiters: ReasoningDelimiterFilter::default(),
            seen_call_ids: BTreeSet::new(),
            seen_call_fingerprints: BTreeSet::new(),
            terminal: false,
        })
    }

    /// Consumes an arbitrary transport fragment and emits complete new events.
    #[must_use]
    pub fn push(&mut self, bytes: &[u8]) -> Vec<GenerationEvent> {
        if self.terminal {
            return Vec::new();
        }
        let mut events = Vec::new();
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > MAX_STREAM_BUFFER_BYTES {
            self.fail(QwenStreamError::InputTooLarge, &mut events);
            return events;
        }
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = self.buffer.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if let Err(error) = self.process_line(&line, &mut events) {
                self.fail(error, &mut events);
                break;
            }
            if self.terminal {
                break;
            }
        }
        events
    }

    /// Ends the transport. A native `done` frame is required for success.
    #[must_use]
    pub fn finish(&mut self, _reason: GenerationCompletion) -> Vec<GenerationEvent> {
        if self.terminal {
            return Vec::new();
        }
        let mut events = Vec::new();
        if !self.buffer.iter().all(u8::is_ascii_whitespace) {
            let line = std::mem::take(&mut self.buffer);
            if let Err(error) = self.process_line(&line, &mut events) {
                self.fail(error, &mut events);
                return events;
            }
        }
        if !self.terminal {
            self.fail(QwenStreamError::UnexpectedEnd, &mut events);
        }
        events
    }

    /// Emits the single cancellation terminal event.
    #[must_use]
    pub fn cancel(&mut self) -> Vec<GenerationEvent> {
        self.external_failure(QwenStreamError::Cancelled)
    }

    /// Emits one sanitized upstream-failure terminal event.
    #[must_use]
    pub fn fail_upstream(&mut self) -> Vec<GenerationEvent> {
        self.external_failure(QwenStreamError::UpstreamFailure)
    }

    /// Emits one memory-pressure terminal event.
    #[must_use]
    pub fn fail_memory_pressure(&mut self) -> Vec<GenerationEvent> {
        self.external_failure(QwenStreamError::MemoryPressure)
    }

    fn process_line(
        &mut self,
        line: &[u8],
        events: &mut Vec<GenerationEvent>,
    ) -> Result<(), QwenStreamError> {
        self.decoded_bytes = self
            .decoded_bytes
            .checked_add(line.len())
            .ok_or(QwenStreamError::InputTooLarge)?;
        if self.decoded_bytes > MAX_DECODED_BYTES {
            return Err(QwenStreamError::InputTooLarge);
        }
        let text = std::str::from_utf8(line).map_err(|_| QwenStreamError::InvalidUtf8)?;
        let frame: NativeFrame =
            serde_json::from_str(text).map_err(|_| QwenStreamError::InvalidProtocol)?;
        if frame.error.is_some() {
            return Err(QwenStreamError::UpstreamFailure);
        }
        if frame.done_reason.is_some() && !frame.done {
            return Err(QwenStreamError::InvalidProtocol);
        }
        let metrics = frame.metrics();
        if let Some(message) = frame.message {
            if message.role != "assistant" {
                return Err(QwenStreamError::InvalidProtocol);
            }
            self.emit_delta(Channel::Reasoning, &message.thinking, events)?;
            self.emit_delta(Channel::Content, &message.content, events)?;
            for call in message.tool_calls {
                self.queue_tool_call(call)?;
            }
        }
        if frame.done {
            if let Some(metrics) = metrics {
                events.push(GenerationEvent::Metrics { metrics });
            }
            self.complete(frame.done_reason.as_deref(), events)?;
        }
        Ok(())
    }

    fn queue_tool_call(&mut self, native: NativeToolCall) -> Result<(), QwenStreamError> {
        if self.pending_calls.len() >= MAX_TOOL_CALLS {
            return Err(QwenStreamError::TooManyToolCalls);
        }
        validate_name(&native.function.name)?;
        let Value::Object(arguments) = native.function.arguments else {
            return Err(QwenStreamError::MalformedToolCall);
        };
        validate_json(&Value::Object(arguments.clone()), 0)?;
        let argument_bytes =
            serde_json::to_vec(&arguments).map_err(|_| QwenStreamError::MalformedToolCall)?;
        if argument_bytes.len() > MAX_ARGUMENT_BYTES {
            return Err(QwenStreamError::InputTooLarge);
        }
        let mut fingerprint = native.function.name.as_bytes().to_vec();
        fingerprint.push(0);
        fingerprint.extend_from_slice(&argument_bytes);
        if !self.seen_call_fingerprints.insert(fingerprint) {
            return Err(QwenStreamError::RepeatedToolCall);
        }
        let call_id = match native.id {
            Some(id) => {
                validate_call_id(&id)?;
                id
            }
            None => self.next_call_id()?,
        };
        if !self.seen_call_ids.insert(call_id.clone()) {
            return Err(QwenStreamError::RepeatedToolCall);
        }
        self.pending_calls.push(ParsedToolCall {
            call_id,
            recipient: native.function.name.clone(),
            name: native.function.name,
            arguments: arguments.into_iter().collect::<BTreeMap<_, _>>(),
        });
        Ok(())
    }

    fn complete(
        &mut self,
        done_reason: Option<&str>,
        events: &mut Vec<GenerationEvent>,
    ) -> Result<(), QwenStreamError> {
        let reason = match done_reason.unwrap_or("stop") {
            "stop" => GenerationCompletion::Stop,
            "length" => GenerationCompletion::Length,
            _ => return Err(QwenStreamError::InvalidProtocol),
        };
        let trailing_content = self.content_delimiters.finish();
        self.pending_content.push_str(&trailing_content);
        Self::emit_chunks(Channel::Preview, &trailing_content, events);
        let content = std::mem::take(&mut self.pending_content);
        let trailing_reasoning = self.reasoning_delimiters.finish();
        Self::emit_chunks(Channel::Reasoning, &trailing_reasoning, events);
        // Presentation must not rewrite native content as thinking: the next
        // request replays these channels through different template positions.
        Self::emit_chunks(Channel::Content, &content, events);
        events.extend(
            self.pending_calls
                .drain(..)
                .map(|call| GenerationEvent::ToolCall { call }),
        );
        self.terminal = true;
        self.buffer.clear();
        events.push(GenerationEvent::Completed { reason });
        Ok(())
    }

    fn emit_delta(
        &mut self,
        channel: Channel,
        text: &str,
        events: &mut Vec<GenerationEvent>,
    ) -> Result<(), QwenStreamError> {
        self.output_bytes = self
            .output_bytes
            .checked_add(text.len())
            .ok_or(QwenStreamError::OutputTooLarge)?;
        if self.output_bytes > MAX_OUTPUT_BYTES {
            return Err(QwenStreamError::OutputTooLarge);
        }
        if channel == Channel::Content {
            let text = self.content_delimiters.push(text);
            self.pending_content.push_str(&text);
            Self::emit_chunks(Channel::Preview, &text, events);
            return Ok(());
        }
        let text = self.reasoning_delimiters.push(text);
        Self::emit_chunks(channel, &text, events);
        Ok(())
    }

    fn emit_chunks(channel: Channel, text: &str, events: &mut Vec<GenerationEvent>) {
        let mut remaining = text;
        while !remaining.is_empty() {
            let mut split = remaining.len().min(MAX_DELTA_BYTES);
            while !remaining.is_char_boundary(split) {
                split -= 1;
            }
            let (chunk, rest) = remaining.split_at(split);
            events.push(match channel {
                Channel::Preview => GenerationEvent::ContentPreviewDelta {
                    text: chunk.to_owned(),
                },
                Channel::Reasoning => GenerationEvent::ReasoningDelta {
                    text: chunk.to_owned(),
                },
                Channel::Content => GenerationEvent::FinalContentDelta {
                    text: chunk.to_owned(),
                },
            });
            remaining = rest;
        }
    }

    fn next_call_id(&mut self) -> Result<String, QwenStreamError> {
        self.next_call_ordinal = self
            .next_call_ordinal
            .checked_add(1)
            .ok_or(QwenStreamError::TooManyToolCalls)?;
        Ok(format!(
            "{}:qwen-tool:{}",
            self.generation_id, self.next_call_ordinal
        ))
    }

    fn external_failure(&mut self, error: QwenStreamError) -> Vec<GenerationEvent> {
        if self.terminal {
            return Vec::new();
        }
        let mut events = Vec::new();
        self.fail(error, &mut events);
        events
    }

    fn fail(&mut self, error: QwenStreamError, events: &mut Vec<GenerationEvent>) {
        if self.terminal {
            return;
        }
        self.terminal = true;
        self.buffer.clear();
        self.pending_calls.clear();
        if !self.pending_content.is_empty() {
            events.push(GenerationEvent::ContentPreviewReset);
        }
        self.pending_content.clear();
        let failure = ModelOutputFailureKind::from(&error);
        events.push(if failure == ModelOutputFailureKind::Cancelled {
            GenerationEvent::Cancelled
        } else {
            GenerationEvent::Failed { failure }
        });
    }
}

impl ModelOutputDecoder for QwenStreamDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<GenerationEvent> {
        Self::push(self, bytes)
    }

    fn finish(&mut self, reason: GenerationCompletion) -> Vec<GenerationEvent> {
        Self::finish(self, reason)
    }

    fn cancel(&mut self) -> Vec<GenerationEvent> {
        Self::cancel(self)
    }

    fn fail_upstream(&mut self) -> Vec<GenerationEvent> {
        Self::fail_upstream(self)
    }

    fn fail_memory_pressure(&mut self) -> Vec<GenerationEvent> {
        Self::fail_memory_pressure(self)
    }
}

#[derive(Deserialize)]
struct NativeFrame {
    #[serde(default)]
    message: Option<NativeMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    error: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
    #[serde(default)]
    prompt_eval_duration: Option<u64>,
    #[serde(default)]
    eval_duration: Option<u64>,
    #[serde(default)]
    total_duration: Option<u64>,
    #[serde(default)]
    load_duration: Option<u64>,
}

impl NativeFrame {
    fn metrics(&self) -> Option<GenerationMetrics> {
        let metrics = GenerationMetrics {
            prompt_tokens: self.prompt_eval_count,
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            generated_tokens: self.eval_count,
            prompt_evaluation_ns: self.prompt_eval_duration,
            generation_ns: self.eval_duration,
            total_ns: self.total_duration,
            load_ns: self.load_duration,
            peak_memory_bytes: None,
        };
        (metrics != GenerationMetrics::default()).then_some(metrics)
    }
}

#[derive(Deserialize)]
struct NativeMessage {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    thinking: String,
    #[serde(default)]
    tool_calls: Vec<NativeToolCall>,
}

#[derive(Deserialize)]
struct NativeToolCall {
    #[serde(default)]
    id: Option<String>,
    function: NativeFunction,
}

#[derive(Deserialize)]
struct NativeFunction {
    name: String,
    arguments: Value,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Channel {
    Preview,
    Reasoning,
    Content,
}

fn validate_name(value: &str) -> Result<(), QwenStreamError> {
    let valid = !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value.split('.').all(|part| {
            let mut bytes = part.bytes();
            bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        });
    if valid {
        Ok(())
    } else {
        Err(QwenStreamError::MalformedToolCall)
    }
}

fn validate_call_id(value: &str) -> Result<(), QwenStreamError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        Err(QwenStreamError::MalformedToolCall)
    } else {
        Ok(())
    }
}

fn validate_json(value: &Value, depth: usize) -> Result<(), QwenStreamError> {
    if depth > MAX_JSON_DEPTH {
        return Err(QwenStreamError::InputTooLarge);
    }
    match value {
        Value::String(text) => validate_json_text(text),
        Value::Array(values) => values
            .iter()
            .try_for_each(|value| validate_json(value, depth + 1)),
        Value::Object(values) => values.iter().try_for_each(|(name, value)| {
            validate_json_text(name)?;
            validate_json(value, depth + 1)
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

fn validate_json_text(value: &str) -> Result<(), QwenStreamError> {
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        Err(QwenStreamError::MalformedToolCall)
    } else {
        Ok(())
    }
}

/// Content-free failures produced by the Qwen native stream decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QwenStreamError {
    /// The request scope used for generated tool IDs is invalid.
    InvalidGenerationId,
    /// Buffered or aggregate provider input exceeded its bound.
    InputTooLarge,
    /// Reasoning and final content exceeded the output bound.
    OutputTooLarge,
    /// Provider bytes were not valid UTF-8 JSON.
    InvalidUtf8,
    /// An NDJSON frame violated the native chat contract.
    InvalidProtocol,
    /// The transport ended before a native terminal frame.
    UnexpectedEnd,
    /// A structured tool call was malformed or unsafe.
    MalformedToolCall,
    /// A native call ID or complete call was repeated.
    RepeatedToolCall,
    /// The stream exceeded the fixed tool-call count.
    TooManyToolCalls,
    /// Generation was explicitly cancelled.
    Cancelled,
    /// The upstream transport or server failed.
    UpstreamFailure,
    /// The host stopped generation for memory pressure.
    MemoryPressure,
}

impl From<&QwenStreamError> for ModelOutputFailureKind {
    fn from(error: &QwenStreamError) -> Self {
        match error {
            QwenStreamError::InvalidGenerationId => Self::InvalidRequestScope,
            QwenStreamError::InputTooLarge => Self::InputLimit,
            QwenStreamError::OutputTooLarge => Self::OutputLimit,
            QwenStreamError::InvalidUtf8 => Self::InvalidEncoding,
            QwenStreamError::InvalidProtocol => Self::InvalidProtocol,
            QwenStreamError::UnexpectedEnd => Self::UnexpectedEnd,
            QwenStreamError::MalformedToolCall | QwenStreamError::RepeatedToolCall => {
                Self::MalformedToolCall
            }
            QwenStreamError::TooManyToolCalls => Self::ToolCallLimit,
            QwenStreamError::Cancelled => Self::Cancelled,
            QwenStreamError::UpstreamFailure => Self::UpstreamFailure,
            QwenStreamError::MemoryPressure => Self::MemoryPressure,
        }
    }
}

impl fmt::Display for QwenStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidGenerationId => "the generation identifier is invalid",
            Self::InputTooLarge => "the Qwen stream input is too large",
            Self::OutputTooLarge => "the Qwen stream output is too large",
            Self::InvalidUtf8 => "the Qwen stream contains invalid UTF-8",
            Self::InvalidProtocol => "the Qwen stream protocol is invalid",
            Self::UnexpectedEnd => "the Qwen stream ended unexpectedly",
            Self::MalformedToolCall => "the Qwen tool call is malformed",
            Self::RepeatedToolCall => "the Qwen tool call is repeated",
            Self::TooManyToolCalls => "the Qwen stream contains too many tool calls",
            Self::Cancelled => "generation was cancelled",
            Self::UpstreamFailure => "the inference stream failed",
            Self::MemoryPressure => "generation stopped because system memory is low",
        })
    }
}

impl Error for QwenStreamError {}
