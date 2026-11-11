//! Closed native phase labels distinguish acquisition from source qualification.
use chain_evm::contract::generic::GenericError;
use ipc_types::IpcError;

#[derive(Clone, Copy, Debug)]
pub(super) enum Stage {
    SourcePreflight,
    CurrentCohort,
    PrerequisiteConditions,
    TransactionIntent,
    FeeFacts,
    ExecutionCandidate,
    PredictedCompletion,
    PrerequisiteBinding,
    RefreshedExecution,
}

pub(super) fn run<T>(
    stage: Stage,
    operation: impl FnOnce() -> Result<T, GenericError>,
) -> Result<T, IpcError> {
    #[cfg(debug_assertions)]
    let started = std::time::Instant::now();
    let result = operation();
    #[cfg(debug_assertions)]
    if std::env::var_os("MINUTUS_SOURCE_QUALIFICATION_DIAGNOSTICS")
        .is_some_and(|value| value == "1")
    {
        // Only a fixed enum and closed chain error cross this diagnostic boundary.
        eprintln!(
            "workflowQualification stage={stage:?} outcome={:?} elapsedMs={}",
            result.as_ref().map(|_| ()).map_err(|error| *error),
            started.elapsed().as_millis()
        );
    }
    result.map_err(|error| {
        IpcError::invalid_request(format!(
            "Generic contract qualification stopped during {stage:?}: {error}; this does not by itself establish that source changed"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qualification_error_names_the_phase_without_claiming_source_changed() {
        let result = run::<()>(Stage::FeeFacts, || Err(GenericError::SnapshotMismatch));
        let message = result.err().map(|error| error.message).unwrap_or_default();
        assert!(message.contains("FeeFacts"));
        assert!(message.contains("SnapshotMismatch"));
        assert!(message.contains("does not by itself establish that source changed"));
    }
}
