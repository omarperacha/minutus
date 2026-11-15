//! Data-only causal categories; provider, compiler and filesystem text is absent.
use super::super::GenericError;
use solidity_compilation::CompilationError;
use std::{
    fmt,
    time::{Duration, Instant},
};

/// Host-selected source qualification operation that failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceFailureStage {
    /// Packaged compiler and public cache setup.
    ProviderSetup,
    /// Exact runtime-keyed artifact reuse within the same provider only.
    ArtifactCache,
    /// Public source response acquisition, including its body.
    SourceHttp,
    /// Public source JSON correspondence and compiler input validation.
    SourceInput,
    /// Official compiler cache access or persistence.
    CompilerCache,
    /// Official compiler download acquisition.
    CompilerHttp,
    /// Pinned local compiler execution and output qualification.
    CompilerExecution,
    /// Complete runtime comparison after reproduction.
    RuntimeComparison,
}

/// Closed source failure reason, without untrusted text or request values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceFailureReason {
    /// HTTP error positively identified as a timeout by the HTTP library.
    HttpTimeout,
    /// HTTP error positively identified as connection establishment failure.
    HttpConnect,
    /// Other HTTP client/send failure; no timeout inference is made.
    HttpTransport,
    /// A non-success HTTP response was returned.
    HttpStatus,
    /// Announced HTTP content length exceeds the fixed limit.
    HttpContentLength,
    /// Reading the HTTP body failed without a recognised timeout cause.
    HttpBodyRead,
    /// Response bytes are not valid JSON.
    JsonDecode,
    /// Required source correspondence or input fields do not match the schema.
    SourceSchema,
    /// A public cache filesystem operation failed.
    CacheIo,
    /// A cache path or file has an unsupported shape.
    CacheLocation,
    /// The target address is invalid.
    InvalidTarget,
    /// A closed exact local compiler failure.
    Compiler(CompilationError),
    /// Reproduced and proof-checked complete runtime bytes disagree.
    RuntimeMismatch,
}

/// Failure facts only: no source, endpoint, path, credential or authority handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceFailure {
    stage: SourceFailureStage,
    reason: SourceFailureReason,
    elapsed_ms: u64,
    stage_elapsed_ms: u64,
    http_status: Option<u16>,
}
impl SourceFailure {
    /// Host-selected operation that failed.
    #[must_use]
    pub const fn stage(self) -> SourceFailureStage {
        self.stage
    }
    /// Closed reason with no underlying error prose.
    #[must_use]
    pub const fn reason(self) -> SourceFailureReason {
        self.reason
    }
    /// Elapsed time since this artifact acquisition (or provider setup) began.
    /// Model inference and earlier native consent are outside this clock.
    #[must_use]
    pub const fn elapsed_ms(self) -> u64 {
        self.elapsed_ms
    }
    /// Elapsed time in the failed operation, separately from total acquisition.
    /// Both durations saturate at `u32::MAX` milliseconds.
    #[must_use]
    pub const fn stage_elapsed_ms(self) -> u64 {
        self.stage_elapsed_ms
    }
    /// Native response status, present only for a rejected HTTP status.
    #[must_use]
    pub const fn http_status(self) -> Option<u16> {
        self.http_status
    }
}

impl fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "stage={} reason={} elapsedMs={} stageElapsedMs={}",
            self.stage.code(),
            self.reason.code(),
            self.elapsed_ms,
            self.stage_elapsed_ms
        )?;
        if let Some(status) = self.http_status {
            write!(f, " httpStatus={status}")?;
        }
        Ok(())
    }
}
impl SourceFailureStage {
    /// Stable diagnostic label, independent of provider text.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ProviderSetup => "providerSetup",
            Self::ArtifactCache => "artifactCache",
            Self::SourceHttp => "sourceHttp",
            Self::SourceInput => "sourceInput",
            Self::CompilerCache => "compilerCache",
            Self::CompilerHttp => "compilerHttp",
            Self::CompilerExecution => "compilerExecution",
            Self::RuntimeComparison => "runtimeComparison",
        }
    }
}
impl SourceFailureReason {
    /// Stable closed diagnostic label. Compiler codes remain data-free.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::HttpTimeout => "httpTimeout",
            Self::HttpConnect => "httpConnect",
            Self::HttpTransport => "httpTransport",
            Self::HttpStatus => "httpStatus",
            Self::HttpContentLength => "httpContentLength",
            Self::HttpBodyRead => "httpBodyRead",
            Self::JsonDecode => "jsonDecode",
            Self::SourceSchema => "sourceSchema",
            Self::CacheIo => "cacheIo",
            Self::CacheLocation => "cacheLocation",
            Self::InvalidTarget => "invalidTarget",
            Self::RuntimeMismatch => "runtimeMismatch",
            Self::Compiler(error) => match error {
                CompilationError::UnsupportedCompiler => "compilerUnsupported",
                CompilationError::WorkerUnavailable => "compilerWorkerUnavailable",
                CompilationError::CompilerMismatch => "compilerChecksumMismatch",
                CompilationError::InvalidInput => "compilerInvalidInput",
                CompilationError::CompilationFailed => "compilerCompilationFailed",
                CompilationError::ArtifactMismatch => "compilerArtifactMismatch",
                CompilationError::UnconfirmedTermination => "compilerUnconfirmedTermination",
                CompilationError::Limit => "compilerResourceLimit",
                CompilationError::Cancelled => "compilerCancelled",
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct FailureClock {
    attempt: Instant,
    active: Instant,
    stage: SourceFailureStage,
}
impl FailureClock {
    pub(super) fn start(attempt: Instant, stage: SourceFailureStage) -> Self {
        Self {
            attempt,
            active: Instant::now(),
            stage,
        }
    }
    pub(super) fn fail(self, reason: SourceFailureReason) -> GenericError {
        self.report(reason, None)
    }
    pub(super) fn http_status(self, status: reqwest::StatusCode) -> GenericError {
        self.report(SourceFailureReason::HttpStatus, Some(status.as_u16()))
    }
    fn report(self, reason: SourceFailureReason, http_status: Option<u16>) -> GenericError {
        let failure = SourceFailure {
            stage: self.stage,
            reason,
            elapsed_ms: millis(self.attempt.elapsed()),
            stage_elapsed_ms: millis(self.active.elapsed()),
            http_status,
        };
        #[cfg(debug_assertions)]
        if diagnostics_enabled() {
            eprintln!("sourceQualificationDiagnostic outcome=failed {failure}");
        }
        GenericError::SourceQualificationFailed(failure)
    }
    pub(super) fn success(self) {
        #[cfg(debug_assertions)]
        if diagnostics_enabled() {
            eprintln!(
                "sourceQualificationDiagnostic stage={} outcome=ok elapsedMs={} stageElapsedMs={}",
                self.stage.code(),
                millis(self.attempt.elapsed()),
                millis(self.active.elapsed())
            );
        }
    }
}
#[cfg(debug_assertions)]
fn diagnostics_enabled() -> bool {
    std::env::var_os("MINUTUS_SOURCE_QUALIFICATION_DIAGNOSTICS").is_some_and(|value| value == "1")
}
fn millis(duration: Duration) -> u64 {
    u64::from(u32::try_from(duration.as_millis()).unwrap_or(u32::MAX))
}

pub(super) fn http_reason(error: &reqwest::Error) -> SourceFailureReason {
    if error.is_timeout() {
        SourceFailureReason::HttpTimeout
    } else if error.is_connect() {
        SourceFailureReason::HttpConnect
    } else {
        SourceFailureReason::HttpTransport
    }
}
pub(super) fn body_reason(error: &std::io::Error) -> SourceFailureReason {
    // reqwest's blocking response wraps its data-free classification in io::Error.
    // Do not infer timeout from elapsed time, error text or a generic IO kind.
    error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<reqwest::Error>())
        .filter(|cause| cause.is_timeout())
        .map_or(SourceFailureReason::HttpBodyRead, |_| {
            SourceFailureReason::HttpTimeout
        })
}
