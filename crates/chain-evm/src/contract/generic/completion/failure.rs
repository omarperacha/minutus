//! Closed post-inclusion observations; these facts confer no authority.
use super::GenericError;
use evm_simulation::{SimulationControl, SimulationError};
use std::{fmt, time::Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    ConditionCollection,
    ReceiptAnchorRecheck,
    FinalCurrentAnchor,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OriginalError {
    Generic(GenericError),
    Simulation(SimulationError),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Budget {
    Active(u32),
    Elapsed,
    Cancelled,
    Unavailable,
}

pub(super) struct Recorder<'a, 'cancel> {
    control: &'a SimulationControl<'cancel>,
    diagnostic: &'a mut Option<CompletionDiagnostic>,
}
impl<'a, 'cancel> Recorder<'a, 'cancel> {
    pub(super) fn new(
        control: &'a SimulationControl<'cancel>,
        diagnostic: &'a mut Option<CompletionDiagnostic>,
    ) -> Self {
        Self {
            control,
            diagnostic,
        }
    }
    pub(super) const fn control(&self) -> &'a SimulationControl<'cancel> {
        self.control
    }
    pub(super) fn generic<T>(
        &mut self,
        phase: Phase,
        operation: impl FnOnce() -> Result<T, GenericError>,
    ) -> Result<T, GenericError> {
        let started = Instant::now();
        operation().inspect_err(|error| {
            *self.diagnostic = Some(CompletionDiagnostic::generic(
                phase,
                *error,
                started,
                self.control,
            ));
        })
    }
    pub(super) fn simulation<T>(
        &mut self,
        phase: Phase,
        operation: impl FnOnce() -> Result<T, SimulationError>,
    ) -> Result<T, SimulationError> {
        let started = Instant::now();
        operation().inspect_err(|error| {
            *self.diagnostic = Some(CompletionDiagnostic::simulation(
                phase,
                *error,
                started,
                self.control,
            ));
        })
    }
}

/// Closed failure phase, original error category and the original outer
/// control's state when the error was handled. Coinciding expiry does not
/// establish root cause or prove that a provider failure was a timeout.
/// No provider prose, body, endpoint, signed-worker data or authority is held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionDiagnostic {
    phase: Phase,
    original: OriginalError,
    phase_elapsed_ms: u32,
    budget: Budget,
}
impl CompletionDiagnostic {
    pub(super) fn generic(
        phase: Phase,
        error: GenericError,
        started: Instant,
        control: &SimulationControl<'_>,
    ) -> Self {
        Self::new(phase, OriginalError::Generic(error), started, control)
    }
    pub(super) fn simulation(
        phase: Phase,
        error: SimulationError,
        started: Instant,
        control: &SimulationControl<'_>,
    ) -> Self {
        Self::new(phase, OriginalError::Simulation(error), started, control)
    }
    fn new(
        phase: Phase,
        original: OriginalError,
        started: Instant,
        control: &SimulationControl<'_>,
    ) -> Self {
        let budget = match control.remaining() {
            Ok(left) => Budget::Active(u32::try_from(left.as_millis()).unwrap_or(u32::MAX)),
            Err(SimulationError::Limit) => Budget::Elapsed,
            Err(SimulationError::Cancelled) => Budget::Cancelled,
            Err(_) => Budget::Unavailable,
        };
        Self {
            phase,
            original,
            phase_elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
            budget,
        }
    }
}
impl fmt::Display for CompletionDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "completionDiagnostic phase={:?} originalError=",
            self.phase
        )?;
        match self.original {
            OriginalError::Simulation(error) => write!(f, "simulation:{error:?}")?,
            OriginalError::Generic(error) => write!(f, "generic:{}", generic_code(error))?,
        }
        write!(
            f,
            " phaseElapsedMs={} originalControl=",
            self.phase_elapsed_ms
        )?;
        match self.budget {
            Budget::Active(left) => write!(f, "active remainingMs={left}"),
            Budget::Elapsed => f.write_str("elapsed"),
            Budget::Cancelled => f.write_str("cancelled"),
            Budget::Unavailable => f.write_str("unavailable"),
        }
    }
}
fn generic_code(error: GenericError) -> &'static str {
    match error {
        GenericError::InvalidAbi => "InvalidAbi",
        GenericError::SourceMismatch => "SourceMismatch",
        GenericError::SourceQualificationFailed(_) => "SourceQualificationFailed",
        GenericError::MissingState => "MissingState",
        GenericError::SnapshotMismatch => "SnapshotMismatch",
        GenericError::OpaqueEffects => "OpaqueEffects",
        GenericError::UnsupportedProxy => "UnsupportedProxy",
        GenericError::Limit => "Limit",
        GenericError::ExecutionRejected => "ExecutionRejected",
        GenericError::ConditionUnsatisfied => "ConditionUnsatisfied",
        GenericError::Expired => "Expired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    #[test]
    fn budget_is_an_observation_not_a_timeout_inference_and_text_is_bounded() {
        for budget in [
            Budget::Active(u32::MAX),
            Budget::Elapsed,
            Budget::Cancelled,
            Budget::Unavailable,
        ] {
            let diagnostic = CompletionDiagnostic {
                phase: Phase::ReceiptAnchorRecheck,
                original: OriginalError::Simulation(SimulationError::StateUnavailable),
                phase_elapsed_ms: u32::MAX,
                budget,
            };
            let text = diagnostic.to_string();
            assert!(text.len() < 384);
            assert!(text.contains("originalError=simulation:StateUnavailable"));
            assert!(!text.contains("timeout"));
        }
        let cancelled = AtomicBool::new(true);
        let control = SimulationControl::start(&cancelled);
        let diagnostic = CompletionDiagnostic::generic(
            Phase::FinalCurrentAnchor,
            GenericError::SnapshotMismatch,
            Instant::now(),
            &control,
        );
        assert!(diagnostic.to_string().contains("originalControl=cancelled"));
    }
}
