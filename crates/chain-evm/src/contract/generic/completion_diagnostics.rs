//! Opt-in closed verification stages; observation never changes a result/control.
use super::GenericError;
use evm_simulation::{SimulationControl, SimulationError};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub(super) enum Stage {
    ReceiptHeader,
    ExactInclusion,
    CandidateIdentity,
    CompletionConditions,
    ConditionCohortSelection,
    ConditionReaderSetup,
    SourceStateRead,
    ConditionRead,
    ConditionComparison,
    ConditionStateFinish,
    SourceIdentity,
    ReceiptAnchorRecheck,
    CompletionCurrentAnchor,
    LiveHeadRecheck,
    LiveAnchorRecheck,
    ReaderQuery,
    ReaderCanonicalFinish,
    ConditionSeal,
}

pub(super) fn generic<T>(
    stage: Stage,
    control: &SimulationControl<'_>,
    operation: impl FnOnce() -> Result<T, GenericError>,
) -> Result<T, GenericError> {
    let started = Instant::now();
    let result = operation();
    #[cfg(debug_assertions)]
    emit(
        stage,
        control,
        started,
        result.as_ref().err().copied().map(Failure::Generic),
    );
    #[cfg(not(debug_assertions))]
    let _ = (stage, control, started);
    result
}

pub(super) fn simulation<T>(
    stage: Stage,
    control: &SimulationControl<'_>,
    operation: impl FnOnce() -> Result<T, SimulationError>,
) -> Result<T, SimulationError> {
    let started = Instant::now();
    let result = operation();
    #[cfg(debug_assertions)]
    emit(
        stage,
        control,
        started,
        result.as_ref().err().copied().map(Failure::Simulation),
    );
    #[cfg(not(debug_assertions))]
    let _ = (stage, control, started);
    result
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug)]
enum Failure {
    Generic(GenericError),
    Simulation(SimulationError),
}

#[cfg(debug_assertions)]
fn line(
    stage: Stage,
    elapsed: std::time::Duration,
    remaining: Result<std::time::Duration, SimulationError>,
    failure: Option<Failure>,
) -> String {
    let elapsed = u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX);
    let budget = match remaining {
        Ok(left) => format!(
            "active remainingMs={}",
            u32::try_from(left.as_millis()).unwrap_or(u32::MAX)
        ),
        Err(SimulationError::Limit) => "elapsed".into(),
        Err(SimulationError::Cancelled) => "cancelled".into(),
        Err(_) => "unavailable".into(),
    };
    let outcome = match failure {
        None => "ok".into(),
        Some(Failure::Generic(error)) => format!("generic:{error:?}"),
        Some(Failure::Simulation(error)) => format!("simulation:{error:?}"),
    };
    format!(
        "completionDiagnostic stage={stage:?} outcome={outcome} stageElapsedMs={elapsed} control={budget}"
    )
}

#[cfg(debug_assertions)]
fn emit(stage: Stage, control: &SimulationControl<'_>, started: Instant, failure: Option<Failure>) {
    if std::env::var_os("MINUTUS_SOURCE_QUALIFICATION_DIAGNOSTICS")
        .is_some_and(|value| value == "1")
    {
        eprintln!(
            "{}",
            line(stage, started.elapsed(), control.remaining(), failure)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::atomic::AtomicBool, time::Duration};

    #[test]
    fn observation_preserves_original_results_and_does_not_renew_controls() {
        let cancelled = AtomicBool::new(false);
        let control = SimulationControl::start(&cancelled);
        let before = control.remaining();
        assert_eq!(generic(Stage::ExactInclusion, &control, || Ok(7)), Ok(7));
        assert_eq!(
            generic::<()>(Stage::SourceIdentity, &control, || Err(
                GenericError::SourceMismatch
            )),
            Err(GenericError::SourceMismatch)
        );
        assert_eq!(
            simulation::<()>(Stage::ReceiptAnchorRecheck, &control, || Err(
                SimulationError::Limit
            )),
            Err(SimulationError::Limit)
        );
        assert!(control.remaining().ok() <= before.ok());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn closed_phase_and_budget_distinguish_limits_without_provider_data() {
        assert_eq!(
            line(
                Stage::ReceiptAnchorRecheck,
                Duration::from_millis(17),
                Err(SimulationError::Limit),
                Some(Failure::Simulation(SimulationError::Limit))
            ),
            "completionDiagnostic stage=ReceiptAnchorRecheck outcome=simulation:Limit stageElapsedMs=17 control=elapsed"
        );
        let message = line(
            Stage::ConditionComparison,
            Duration::MAX,
            Ok(Duration::from_secs(2)),
            Some(Failure::Generic(GenericError::ConditionUnsatisfied)),
        );
        assert!(message.contains("ConditionUnsatisfied"));
        assert!(message.contains("stageElapsedMs=4294967295"));
        assert!(message.contains("control=active remainingMs=2000"));
    }
}
