//! Persistence error types.

use std::{error::Error, fmt};

/// Failure returned by the Rust-owned application store.
#[derive(Debug)]
pub enum StorageError {
    /// `SQLite` could not complete an operation.
    Database(rusqlite::Error),
    /// A structured record could not be serialized or decoded.
    Serialization(serde_json::Error),
    /// The database belongs to another application.
    ForeignDatabase,
    /// The database schema is newer than this application understands.
    IncompatibleSchema {
        /// Version found on disk.
        found: u32,
        /// Latest version supported by this application.
        supported: u32,
    },
    /// The database failed an integrity or required-schema check.
    CorruptDatabase,
    /// A record violated an application-level invariant.
    InvalidRecord(&'static str),
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(_) => formatter.write_str("local database operation failed"),
            Self::Serialization(_) => formatter.write_str("stored record could not be decoded"),
            Self::ForeignDatabase => {
                formatter.write_str("database is not a Minutus application database")
            }
            Self::IncompatibleSchema { found, supported } => write!(
                formatter,
                "database schema version {found} is newer than supported version {supported}"
            ),
            Self::CorruptDatabase => formatter.write_str("local database is corrupt or incomplete"),
            Self::InvalidRecord(reason) => {
                write!(formatter, "invalid persistence record: {reason}")
            }
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::ForeignDatabase
            | Self::IncompatibleSchema { .. }
            | Self::CorruptDatabase
            | Self::InvalidRecord(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}
