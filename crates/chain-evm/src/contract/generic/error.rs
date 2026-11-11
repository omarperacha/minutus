use std::{error::Error, fmt};

/// Closed generic admission failures. Source/provider/worker prose is excluded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenericError {
    /// The compiled ABI does not contain one unambiguous canonical invocation.
    InvalidAbi,
    /// Source/compiler/runtime correspondence cannot be established, including
    /// unavailable acquisition or compiler execution and actual byte mismatch.
    SourceMismatch,
    /// A required account, slot or code identity is absent from the witness.
    MissingState,
    /// A coherent source, call, context, envelope or canonical snapshot could
    /// not be established, including unavailable acquisition. This error alone
    /// does not establish that source code changed.
    SnapshotMismatch,
    /// An executed effect has no complete supported structural interpretation.
    OpaqueEffects,
    /// Proxy dispatch, control mutation or delegation is unsupported.
    UnsupportedProxy,
    /// A bounded collection, code graph or execution limit was exceeded.
    Limit,
    /// The complete trusted execution did not succeed.
    ExecutionRejected,
    /// A fully proved typed postcondition is false; this is distinct from an
    /// unavailable proof or uncertain transaction inclusion.
    ConditionUnsatisfied,
    /// The host cancelled or the evidence expired.
    Expired,
}
impl fmt::Display for GenericError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Generic contract admission rejected: {self:?}")
    }
}
impl Error for GenericError {}
