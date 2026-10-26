//! Adapter-neutral model output failure classification.

/// Closed failure kinds shared by model output adapters and downstream diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelOutputFailureKind {
    /// The request-scoped generation identity was invalid.
    InvalidRequestScope,
    /// Adapter input exceeded its fixed safety bound.
    InputLimit,
    /// Parsed model output exceeded its fixed safety bound.
    OutputLimit,
    /// Model output was not valid in the expected character encoding.
    InvalidEncoding,
    /// Model output violated the selected adapter protocol.
    InvalidProtocol,
    /// The output stream ended before a valid protocol boundary.
    UnexpectedEnd,
    /// A tool or channel recipient was malformed.
    InvalidRecipient,
    /// A model-authored tool invocation was malformed.
    MalformedToolCall,
    /// A tool recipient and invoked function did not match.
    RecipientMismatch,
    /// A tool invocation repeated one parameter.
    DuplicateToolParameter,
    /// A generation exceeded the bounded number of tool calls.
    ToolCallLimit,
    /// Generation was explicitly cancelled.
    Cancelled,
    /// The upstream inference stream failed.
    UpstreamFailure,
    /// The inference provider exceeded its bounded generation deadline.
    ProviderTimeout,
    /// The inference provider disconnected or became unavailable.
    ProviderUnavailable,
    /// The inference provider rejected the bounded request.
    ProviderRejected,
    /// The inference provider returned malformed or oversized transport data.
    InvalidProviderResponse,
    /// Inference stopped to protect system responsiveness.
    MemoryPressure,
}

impl ModelOutputFailureKind {
    /// Stable adapter-neutral code for private development diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidRequestScope => "invalid_request_scope",
            Self::InputLimit => "input_limit",
            Self::OutputLimit => "output_limit",
            Self::InvalidEncoding => "invalid_encoding",
            Self::InvalidProtocol => "invalid_protocol",
            Self::UnexpectedEnd => "unexpected_end",
            Self::InvalidRecipient => "invalid_recipient",
            Self::MalformedToolCall => "malformed_tool_call",
            Self::RecipientMismatch => "recipient_mismatch",
            Self::DuplicateToolParameter => "duplicate_tool_parameter",
            Self::ToolCallLimit => "tool_call_limit",
            Self::Cancelled => "cancelled",
            Self::UpstreamFailure => "upstream_failure",
            Self::ProviderTimeout => "provider_timeout",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderRejected => "provider_rejected",
            Self::InvalidProviderResponse => "invalid_provider_response",
            Self::MemoryPressure => "memory_pressure",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glimmer::GlimmerStreamError;

    #[test]
    fn glimmer_failures_map_exhaustively_to_adapter_neutral_codes() {
        let cases = [
            (
                GlimmerStreamError::InvalidGenerationId,
                ModelOutputFailureKind::InvalidRequestScope,
                "invalid_request_scope",
            ),
            (
                GlimmerStreamError::InputTooLarge,
                ModelOutputFailureKind::InputLimit,
                "input_limit",
            ),
            (
                GlimmerStreamError::OutputTooLarge,
                ModelOutputFailureKind::OutputLimit,
                "output_limit",
            ),
            (
                GlimmerStreamError::InvalidUtf8,
                ModelOutputFailureKind::InvalidEncoding,
                "invalid_encoding",
            ),
            (
                GlimmerStreamError::UnexpectedProtocol,
                ModelOutputFailureKind::InvalidProtocol,
                "invalid_protocol",
            ),
            (
                GlimmerStreamError::UnexpectedEnd,
                ModelOutputFailureKind::UnexpectedEnd,
                "unexpected_end",
            ),
            (
                GlimmerStreamError::InvalidRecipient,
                ModelOutputFailureKind::InvalidRecipient,
                "invalid_recipient",
            ),
            (
                GlimmerStreamError::MalformedToolCall,
                ModelOutputFailureKind::MalformedToolCall,
                "malformed_tool_call",
            ),
            (
                GlimmerStreamError::RecipientMismatch,
                ModelOutputFailureKind::RecipientMismatch,
                "recipient_mismatch",
            ),
            (
                GlimmerStreamError::DuplicateParameter,
                ModelOutputFailureKind::DuplicateToolParameter,
                "duplicate_tool_parameter",
            ),
            (
                GlimmerStreamError::TooManyToolCalls,
                ModelOutputFailureKind::ToolCallLimit,
                "tool_call_limit",
            ),
            (
                GlimmerStreamError::Cancelled,
                ModelOutputFailureKind::Cancelled,
                "cancelled",
            ),
            (
                GlimmerStreamError::UpstreamFailure,
                ModelOutputFailureKind::UpstreamFailure,
                "upstream_failure",
            ),
            (
                GlimmerStreamError::MemoryPressure,
                ModelOutputFailureKind::MemoryPressure,
                "memory_pressure",
            ),
        ];
        for (error, expected_kind, expected_code) in cases {
            let kind = ModelOutputFailureKind::from(&error);
            assert_eq!(kind, expected_kind);
            assert_eq!(kind.code(), expected_code);
        }
    }
}
