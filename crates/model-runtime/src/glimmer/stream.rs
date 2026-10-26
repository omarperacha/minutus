//! Incremental parsing of Glimmer recipient channels and ATEM tool calls.

use std::{error::Error, fmt, str};

use crate::{
    GenerationCompletion, GenerationEvent, ModelOutputDecoder, ModelOutputFailureKind,
    ParsedToolCall,
};

mod atem;

use super::validate::is_valid_recipient;
use atem::parse_atem;

const ANCHOR: &str = "<|start|>assistant";
const RECIPIENT_PREFIX: &str = " to=";
const MESSAGE: &str = "<|message|>";
const EOM: &str = "<|eom|>";
const EOT: &str = "<|eot|>";
const MAX_DELTA_BYTES: usize = 32 * 1024;
const MAX_STREAM_BUFFER_BYTES: usize = 512 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_GENERATION_ID_BYTES: usize = 128;

/// Stateful incremental Glimmer output parser.
pub(crate) struct GlimmerStreamParser {
    generation_id: String,
    next_call_ordinal: u32,
    state: ParserState,
    pending_bytes: Vec<u8>,
    buffer: String,
    emitted_bytes: usize,
    terminal: bool,
}

impl ModelOutputDecoder for GlimmerStreamParser {
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

impl GlimmerStreamParser {
    /// Creates a parser whose generated tool IDs are scoped to one request.
    ///
    /// # Errors
    ///
    /// Returns an error if the generation identifier is empty, excessive or
    /// contains characters unsuitable for an opaque tool-call identifier.
    pub(crate) fn new(generation_id: impl Into<String>) -> Result<Self, GlimmerStreamError> {
        let generation_id = generation_id.into();
        if generation_id.is_empty()
            || generation_id.len() > MAX_GENERATION_ID_BYTES
            || !generation_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
        {
            return Err(GlimmerStreamError::InvalidGenerationId);
        }
        Ok(Self {
            generation_id,
            next_call_ordinal: 0,
            state: ParserState::Header {
                require_anchor: false,
            },
            pending_bytes: Vec::new(),
            buffer: String::new(),
            emitted_bytes: 0,
            terminal: false,
        })
    }

    /// Consumes one arbitrary byte chunk and returns every newly available event.
    ///
    /// Incomplete UTF-8 and protocol fragments are retained until a later call.
    /// After a terminal event, further input produces no events.
    #[must_use]
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<GenerationEvent> {
        if self.terminal {
            return Vec::new();
        }
        let mut events = Vec::new();
        // Process even unusually large transport chunks incrementally. This
        // keeps ordinary text streaming while retaining a strict bound for a
        // protocol construct, such as an unterminated ATEM call.
        for chunk in bytes.chunks(MAX_DELTA_BYTES) {
            self.pending_bytes.extend_from_slice(chunk);
            if let Err(error) = self.decode_and_process(&mut events) {
                self.fail(error, &mut events);
                break;
            }
        }
        events
    }

    /// Completes the stream with the backend's finish reason.
    ///
    /// Normal stop requires a parsed Glimmer turn terminator. A length limit
    /// may finalize a partial user-visible content channel, but never partial
    /// reasoning, headers, UTF-8 or tool syntax.
    #[must_use]
    pub(crate) fn finish(&mut self, reason: GenerationCompletion) -> Vec<GenerationEvent> {
        if self.terminal {
            return Vec::new();
        }
        let mut events = Vec::new();
        if let Err(error) = self.decode_and_process(&mut events) {
            self.fail(error, &mut events);
            return events;
        }
        if !self.pending_bytes.is_empty() {
            self.fail(GlimmerStreamError::InvalidUtf8, &mut events);
            return events;
        }
        let result = match (&self.state, reason) {
            (ParserState::AwaitingFinish, _) if self.buffer.is_empty() => Ok(()),
            (ParserState::Content, GenerationCompletion::Length) => {
                self.flush_length_limited_content(&mut events)
            }
            _ => Err(GlimmerStreamError::UnexpectedEnd),
        };
        match result {
            Ok(()) => {
                self.state = ParserState::Terminal;
                self.terminal = true;
                events.push(GenerationEvent::Completed { reason });
            }
            Err(error) => self.fail(error, &mut events),
        }
        events
    }

    /// Terminates the generation as cancelled, emitting at most one failure.
    #[must_use]
    pub(crate) fn cancel(&mut self) -> Vec<GenerationEvent> {
        self.external_failure(GlimmerStreamError::Cancelled)
    }

    /// Terminates the generation after a sanitized upstream transport failure.
    #[must_use]
    pub(crate) fn fail_upstream(&mut self) -> Vec<GenerationEvent> {
        self.external_failure(GlimmerStreamError::UpstreamFailure)
    }

    /// Terminates generation because the host reported memory pressure.
    #[must_use]
    pub(crate) fn fail_memory_pressure(&mut self) -> Vec<GenerationEvent> {
        self.external_failure(GlimmerStreamError::MemoryPressure)
    }

    fn external_failure(&mut self, error: GlimmerStreamError) -> Vec<GenerationEvent> {
        if self.terminal {
            return Vec::new();
        }
        let mut events = Vec::new();
        self.fail(error, &mut events);
        events
    }

    fn decode_and_process(
        &mut self,
        events: &mut Vec<GenerationEvent>,
    ) -> Result<(), GlimmerStreamError> {
        loop {
            match str::from_utf8(&self.pending_bytes) {
                Ok(text) => {
                    self.buffer.push_str(text);
                    self.pending_bytes.clear();
                    break;
                }
                Err(error) if error.valid_up_to() > 0 => {
                    let valid_bytes = self.pending_bytes[..error.valid_up_to()].to_vec();
                    let valid = str::from_utf8(&valid_bytes)
                        .map_err(|_| GlimmerStreamError::InvalidUtf8)?;
                    self.buffer.push_str(valid);
                    self.pending_bytes.drain(..error.valid_up_to());
                    self.process_buffer(events)?;
                    if error.error_len().is_some() {
                        return Err(GlimmerStreamError::InvalidUtf8);
                    }
                }
                Err(error) if error.error_len().is_some() => {
                    return Err(GlimmerStreamError::InvalidUtf8);
                }
                Err(_) => break,
            }
        }
        self.process_buffer(events)
    }

    fn process_buffer(
        &mut self,
        events: &mut Vec<GenerationEvent>,
    ) -> Result<(), GlimmerStreamError> {
        if self.buffer.len() > MAX_STREAM_BUFFER_BYTES {
            return Err(GlimmerStreamError::InputTooLarge);
        }
        loop {
            let state = std::mem::replace(&mut self.state, ParserState::Terminal);
            let (next, progressed) = match state {
                ParserState::Header { require_anchor } => self.step_header(require_anchor)?,
                ParserState::Reasoning => self.step_delta(Channel::Reasoning, events)?,
                ParserState::Content => self.step_delta(Channel::Content, events)?,
                ParserState::Tool { recipient } => self.step_tool(recipient, events)?,
                ParserState::AwaitingFinish => {
                    if self.buffer.is_empty() {
                        (ParserState::AwaitingFinish, false)
                    } else {
                        return Err(GlimmerStreamError::UnexpectedProtocol);
                    }
                }
                ParserState::Terminal => (ParserState::Terminal, false),
            };
            self.state = next;
            if !progressed {
                return Ok(());
            }
            if self.buffer.len() > MAX_STREAM_BUFFER_BYTES {
                return Err(GlimmerStreamError::InputTooLarge);
            }
        }
    }

    fn step_header(
        &mut self,
        mut require_anchor: bool,
    ) -> Result<(ParserState, bool), GlimmerStreamError> {
        let original_length = self.buffer.len();
        if require_anchor {
            if self.buffer.len() < ANCHOR.len() && ANCHOR.starts_with(&self.buffer) {
                return Ok((ParserState::Header { require_anchor }, false));
            }
            if !self.buffer.starts_with(ANCHOR) {
                return Err(GlimmerStreamError::UnexpectedProtocol);
            }
            self.buffer.drain(..ANCHOR.len());
            require_anchor = false;
        } else if self.buffer.starts_with(ANCHOR) {
            self.buffer.drain(..ANCHOR.len());
        } else if !self.buffer.is_empty() && ANCHOR.starts_with(&self.buffer) {
            return Ok((ParserState::Header { require_anchor }, false));
        }

        if self.buffer.is_empty() || RECIPIENT_PREFIX.starts_with(&self.buffer) {
            return Ok((
                ParserState::Header { require_anchor },
                original_length != self.buffer.len(),
            ));
        }
        if !self.buffer.starts_with(RECIPIENT_PREFIX) {
            return Err(GlimmerStreamError::UnexpectedProtocol);
        }
        let Some(message_index) = self.buffer.find(MESSAGE) else {
            return Ok((
                ParserState::Header { require_anchor },
                original_length != self.buffer.len(),
            ));
        };
        let recipient = self.buffer[RECIPIENT_PREFIX.len()..message_index].to_owned();
        if !is_valid_recipient(&recipient) {
            return Err(GlimmerStreamError::InvalidRecipient);
        }
        self.buffer.drain(..message_index + MESSAGE.len());
        let next = match recipient.as_str() {
            "self" => ParserState::Reasoning,
            "user" => ParserState::Content,
            _ => ParserState::Tool { recipient },
        };
        Ok((next, true))
    }

    fn step_delta(
        &mut self,
        channel: Channel,
        events: &mut Vec<GenerationEvent>,
    ) -> Result<(ParserState, bool), GlimmerStreamError> {
        let delimiters = match channel {
            Channel::Reasoning => &[(EOM, false)][..],
            Channel::Content => &[(EOM, false), (EOT, true)][..],
        };
        if let Some((index, delimiter, terminal)) = earliest_delimiter(&self.buffer, delimiters) {
            let text = self.buffer[..index].to_owned();
            reject_unexpected_marker(&text)?;
            self.emit_delta(channel, &text, events)?;
            self.buffer.drain(..index + delimiter.len());
            return Ok((
                if terminal {
                    ParserState::AwaitingFinish
                } else {
                    ParserState::Header {
                        require_anchor: true,
                    }
                },
                true,
            ));
        }
        let protected = longest_delimiter_prefix_suffix(&self.buffer, delimiters);
        let safe_length = self.buffer.len() - protected;
        if safe_length == 0 {
            return Ok((channel.state(), false));
        }
        let text = self.buffer[..safe_length].to_owned();
        reject_unexpected_marker(&text)?;
        self.emit_delta(channel, &text, events)?;
        self.buffer.drain(..safe_length);
        Ok((channel.state(), true))
    }

    fn step_tool(
        &mut self,
        recipient: String,
        events: &mut Vec<GenerationEvent>,
    ) -> Result<(ParserState, bool), GlimmerStreamError> {
        let delimiters = &[(EOM, false), (EOT, true)][..];
        let Some((index, delimiter, terminal)) = earliest_delimiter(&self.buffer, delimiters)
        else {
            let protected = longest_delimiter_prefix_suffix(&self.buffer, delimiters);
            let inspect_length = self.buffer.len() - protected;
            reject_unexpected_marker(&self.buffer[..inspect_length])?;
            if self.buffer.len() > MAX_STREAM_BUFFER_BYTES {
                return Err(GlimmerStreamError::InputTooLarge);
            }
            return Ok((ParserState::Tool { recipient }, false));
        };
        let payload = self.buffer[..index].to_owned();
        reject_unexpected_marker(&payload)?;
        let (name, arguments) = parse_atem(&payload)?;
        if name != recipient {
            return Err(GlimmerStreamError::RecipientMismatch);
        }
        let call_id = self.next_call_id()?;
        events.push(GenerationEvent::ToolCall {
            call: ParsedToolCall {
                call_id,
                recipient,
                name,
                arguments,
            },
        });
        self.buffer.drain(..index + delimiter.len());
        Ok((
            if terminal {
                ParserState::AwaitingFinish
            } else {
                ParserState::Header {
                    require_anchor: true,
                }
            },
            true,
        ))
    }

    fn emit_delta(
        &mut self,
        channel: Channel,
        text: &str,
        events: &mut Vec<GenerationEvent>,
    ) -> Result<(), GlimmerStreamError> {
        self.emitted_bytes = self
            .emitted_bytes
            .checked_add(text.len())
            .ok_or(GlimmerStreamError::OutputTooLarge)?;
        if self.emitted_bytes > MAX_OUTPUT_BYTES {
            return Err(GlimmerStreamError::OutputTooLarge);
        }
        let mut remaining = text;
        while !remaining.is_empty() {
            let mut split = remaining.len().min(MAX_DELTA_BYTES);
            while !remaining.is_char_boundary(split) {
                split -= 1;
            }
            let (chunk, rest) = remaining.split_at(split);
            events.push(match channel {
                Channel::Reasoning => GenerationEvent::ReasoningDelta {
                    text: chunk.to_owned(),
                },
                Channel::Content => GenerationEvent::FinalContentDelta {
                    text: chunk.to_owned(),
                },
            });
            remaining = rest;
        }
        Ok(())
    }

    fn flush_length_limited_content(
        &mut self,
        events: &mut Vec<GenerationEvent>,
    ) -> Result<(), GlimmerStreamError> {
        reject_unexpected_marker(&self.buffer)?;
        let text = std::mem::take(&mut self.buffer);
        self.emit_delta(Channel::Content, &text, events)
    }

    fn next_call_id(&mut self) -> Result<String, GlimmerStreamError> {
        self.next_call_ordinal = self
            .next_call_ordinal
            .checked_add(1)
            .ok_or(GlimmerStreamError::TooManyToolCalls)?;
        Ok(format!(
            "{}:tool:{}",
            self.generation_id, self.next_call_ordinal
        ))
    }

    fn fail(&mut self, error: GlimmerStreamError, events: &mut Vec<GenerationEvent>) {
        if self.terminal {
            return;
        }
        self.terminal = true;
        self.state = ParserState::Terminal;
        self.pending_bytes.clear();
        self.buffer.clear();
        let failure = ModelOutputFailureKind::from(&error);
        events.push(if failure == ModelOutputFailureKind::Cancelled {
            GenerationEvent::Cancelled
        } else {
            GenerationEvent::Failed { failure }
        });
    }
}

#[derive(Clone, Copy)]
enum Channel {
    Reasoning,
    Content,
}

impl Channel {
    const fn state(self) -> ParserState {
        match self {
            Self::Reasoning => ParserState::Reasoning,
            Self::Content => ParserState::Content,
        }
    }
}

enum ParserState {
    Header { require_anchor: bool },
    Reasoning,
    Content,
    Tool { recipient: String },
    AwaitingFinish,
    Terminal,
}

fn earliest_delimiter<'a>(
    buffer: &str,
    delimiters: &'a [(&'a str, bool)],
) -> Option<(usize, &'a str, bool)> {
    delimiters
        .iter()
        .filter_map(|(delimiter, terminal)| {
            buffer
                .find(delimiter)
                .map(|index| (index, *delimiter, *terminal))
        })
        .min_by_key(|(index, _, _)| *index)
}

fn longest_delimiter_prefix_suffix(buffer: &str, delimiters: &[(&str, bool)]) -> usize {
    delimiters
        .iter()
        .map(|(delimiter, _)| {
            (1..delimiter.len())
                .rev()
                .find(|length| buffer.ends_with(&delimiter[..*length]))
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}

fn reject_unexpected_marker(text: &str) -> Result<(), GlimmerStreamError> {
    if text.contains("<|") {
        Err(GlimmerStreamError::UnexpectedProtocol)
    } else {
        Ok(())
    }
}

/// Failure while parsing streamed Glimmer output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlimmerStreamError {
    /// The request scope used for generated tool IDs is invalid.
    InvalidGenerationId,
    /// Buffered protocol input exceeded its fixed bound.
    InputTooLarge,
    /// Parsed reasoning/content exceeded its fixed bound.
    OutputTooLarge,
    /// A byte sequence is not valid UTF-8.
    InvalidUtf8,
    /// Glimmer emitted an unexpected channel or control marker.
    UnexpectedProtocol,
    /// The backend ended before a valid protocol boundary.
    UnexpectedEnd,
    /// A channel recipient is unsafe or malformed.
    InvalidRecipient,
    /// An ATEM invocation is malformed or unsupported.
    MalformedToolCall,
    /// The channel recipient and ATEM function name differ.
    RecipientMismatch,
    /// An ATEM invocation repeats a parameter name.
    DuplicateParameter,
    /// The request-scoped tool-call ordinal overflowed.
    TooManyToolCalls,
    /// Generation was explicitly cancelled.
    Cancelled,
    /// The upstream inference stream failed.
    UpstreamFailure,
    /// The host cancelled inference to protect system responsiveness.
    MemoryPressure,
}

impl From<&GlimmerStreamError> for crate::ModelOutputFailureKind {
    fn from(error: &GlimmerStreamError) -> Self {
        match error {
            GlimmerStreamError::InvalidGenerationId => Self::InvalidRequestScope,
            GlimmerStreamError::InputTooLarge => Self::InputLimit,
            GlimmerStreamError::OutputTooLarge => Self::OutputLimit,
            GlimmerStreamError::InvalidUtf8 => Self::InvalidEncoding,
            GlimmerStreamError::UnexpectedProtocol => Self::InvalidProtocol,
            GlimmerStreamError::UnexpectedEnd => Self::UnexpectedEnd,
            GlimmerStreamError::InvalidRecipient => Self::InvalidRecipient,
            GlimmerStreamError::MalformedToolCall => Self::MalformedToolCall,
            GlimmerStreamError::RecipientMismatch => Self::RecipientMismatch,
            GlimmerStreamError::DuplicateParameter => Self::DuplicateToolParameter,
            GlimmerStreamError::TooManyToolCalls => Self::ToolCallLimit,
            GlimmerStreamError::Cancelled => Self::Cancelled,
            GlimmerStreamError::UpstreamFailure => Self::UpstreamFailure,
            GlimmerStreamError::MemoryPressure => Self::MemoryPressure,
        }
    }
}

impl fmt::Display for GlimmerStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGenerationId => {
                formatter.write_str("the generation identifier is invalid")
            }
            Self::InputTooLarge => formatter.write_str("the Glimmer stream buffer is too large"),
            Self::OutputTooLarge => formatter.write_str("the Glimmer output is too large"),
            Self::InvalidUtf8 => formatter.write_str("the Glimmer stream contains invalid UTF-8"),
            Self::UnexpectedProtocol => {
                formatter.write_str("the Glimmer stream protocol is invalid")
            }
            Self::UnexpectedEnd => formatter.write_str("the Glimmer stream ended unexpectedly"),
            Self::InvalidRecipient => formatter.write_str("the Glimmer recipient is invalid"),
            Self::MalformedToolCall => formatter.write_str("the ATEM tool call is malformed"),
            Self::RecipientMismatch => {
                formatter.write_str("the ATEM tool does not match its recipient")
            }
            Self::DuplicateParameter => {
                formatter.write_str("the ATEM tool call repeats a parameter")
            }
            Self::TooManyToolCalls => {
                formatter.write_str("the stream contains too many tool calls")
            }
            Self::Cancelled => formatter.write_str("generation was cancelled"),
            Self::UpstreamFailure => formatter.write_str("the inference stream failed"),
            Self::MemoryPressure => {
                formatter.write_str("generation stopped because system memory is low")
            }
        }
    }
}

impl Error for GlimmerStreamError {}
