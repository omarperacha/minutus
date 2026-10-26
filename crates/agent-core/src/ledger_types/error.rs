use std::fmt;

const MAX_ID_BYTES: usize = 160;
const MAX_PATH_BYTES: usize = 1_024;
const MAX_MESSAGE_BYTES: usize = 2_048;

/// Validation and consistency errors for the trusted execution ledger.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionLedgerError {
    /// A supplied or restored record is malformed.
    InvalidRecord(String),
    /// A fact does not belong to the ledger's current request/project scope.
    ScopeMismatch,
    /// A tool result does not match the exact validated request.
    ResultMismatch,
    /// The required terminal tool outcome was not present.
    WrongResultOutcome,
    /// The requested workspace alias is unresolved.
    UnknownWorkspace(String),
    /// A stable fact or mutation ID was already used.
    DuplicateFact(String),
    /// A bounded ledger collection or serialized snapshot is full.
    LimitExceeded,
}

impl fmt::Display for ExecutionLedgerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRecord(message) => write!(formatter, "invalid ledger record: {message}"),
            Self::ScopeMismatch => formatter.write_str("ledger fact is outside its request scope"),
            Self::ResultMismatch => formatter.write_str("tool result does not match the request"),
            Self::WrongResultOutcome => formatter.write_str("tool result has the wrong outcome"),
            Self::UnknownWorkspace(alias) => write!(formatter, "unknown workspace alias: {alias}"),
            Self::DuplicateFact(id) => write!(formatter, "duplicate ledger fact: {id}"),
            Self::LimitExceeded => formatter.write_str("execution ledger limit was reached"),
        }
    }
}

impl std::error::Error for ExecutionLedgerError {}

pub(crate) fn validate_id(label: &str, value: &str) -> Result<(), ExecutionLedgerError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ExecutionLedgerError::InvalidRecord(format!(
            "{label} is malformed"
        )));
    }
    Ok(())
}

pub(crate) fn validate_alias(value: &str) -> Result<(), ExecutionLedgerError> {
    if value.is_empty()
        || value.len() > 80
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(ExecutionLedgerError::InvalidRecord(
            "workspace alias is malformed".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_relative_path(value: &str) -> Result<(), ExecutionLedgerError> {
    if value.is_empty()
        || value.len() > MAX_PATH_BYTES
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\0')
        || value.split(['/', '\\']).any(|component| component == "..")
    {
        return Err(ExecutionLedgerError::InvalidRecord(
            "relative path is malformed".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_digest(value: &str) -> Result<(), ExecutionLedgerError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ExecutionLedgerError::InvalidRecord(
            "content digest is malformed".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_message(label: &str, value: &str) -> Result<(), ExecutionLedgerError> {
    if value.is_empty() || value.len() > MAX_MESSAGE_BYTES || value.chars().any(char::is_control) {
        return Err(ExecutionLedgerError::InvalidRecord(format!(
            "{label} is malformed"
        )));
    }
    Ok(())
}
