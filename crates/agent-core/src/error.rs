use std::fmt;

/// Closed validation failures produced before execution or result publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolRegistryError {
    /// A trusted registry entry is malformed.
    InvalidDefinition(String),
    /// A trusted JSON Schema is malformed or uses unsupported keywords.
    InvalidSchema {
        /// JSONPath-like location of the malformed schema node.
        path: String,
        /// Sanitized validation reason.
        message: String,
    },
    /// Two trusted entries declare the same stable name.
    DuplicateTool(String),
    /// The model requested a name absent from the active registry.
    UnknownTool(String),
    /// The adapter-provided call ID is malformed.
    InvalidCallId,
    /// Model arguments do not satisfy the exact trusted schema.
    InvalidArguments {
        /// JSONPath-like location of the invalid argument.
        path: String,
        /// Sanitized validation reason.
        message: String,
    },
    /// The registry changed after request validation.
    StaleValidatedRequest,
    /// An executor returned a result type absent from its allow-list.
    UnsupportedResultType,
    /// A result exceeded its entry's byte or part bound.
    ResultLimitExceeded,
    /// A result reference or failure is malformed.
    InvalidResult(String),
}

impl fmt::Display for ToolRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDefinition(message) => {
                write!(formatter, "invalid tool definition: {message}")
            }
            Self::InvalidSchema { path, message } => {
                write!(formatter, "invalid tool schema at {path}: {message}")
            }
            Self::DuplicateTool(name) => write!(formatter, "duplicate tool: {name}"),
            Self::UnknownTool(name) => write!(formatter, "unknown tool: {name}"),
            Self::InvalidCallId => formatter.write_str("invalid tool call ID"),
            Self::InvalidArguments { path, message } => {
                write!(formatter, "invalid tool arguments at {path}: {message}")
            }
            Self::StaleValidatedRequest => formatter.write_str("validated tool request is stale"),
            Self::UnsupportedResultType => {
                formatter.write_str("tool returned an unsupported result type")
            }
            Self::ResultLimitExceeded => {
                formatter.write_str("tool result exceeds its trusted limit")
            }
            Self::InvalidResult(message) => write!(formatter, "invalid tool result: {message}"),
        }
    }
}

impl std::error::Error for ToolRegistryError {}
