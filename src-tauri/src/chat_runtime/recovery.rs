//! Request-local recovery checkpoints and bounded retry decisions.

use std::collections::{BTreeMap, BTreeSet};

use app_core::ToolRecoveryDiagnostic;
use ipc_types::{IpcError, IpcErrorCode};

pub(super) const MAX_RECOVERY_ATTEMPTS: usize = 3;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum RecoveryKind {
    InvalidArguments,
    StalePhase,
    UnavailableTool,
    PlanValidation,
    ForecastValidation,
    OutputExhaustion,
    TransientProvider,
    TransientTool,
}

impl RecoveryKind {
    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::StalePhase => "stale_phase",
            Self::UnavailableTool => "unavailable_tool",
            Self::PlanValidation => "plan_validation",
            Self::ForecastValidation => "forecast_validation",
            Self::OutputExhaustion => "output_exhaustion",
            Self::TransientProvider => "transient_provider",
            Self::TransientTool => "transient_tool",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct RecoveryCheckpoint {
    pub(super) plan_revision: usize,
    pub(super) forecast_revision: usize,
    pub(super) useful_result_sequence: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RecoveryKey {
    kind: RecoveryKind,
    scope: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PlanDefectProgress {
    pub(super) removed: usize,
    pub(super) changed: usize,
    pub(super) newly_visible: usize,
}

#[derive(Clone, Debug, Default)]
struct RecoveryAttempts {
    inputs: BTreeSet<String>,
    total: usize,
}

#[derive(Clone, Debug)]
pub(super) enum RecoveryDecision {
    Resume,
    Terminal(IpcError),
}

#[derive(Default)]
pub(super) struct RecoveryCoordinator {
    checkpoint: RecoveryCheckpoint,
    attempts: BTreeMap<RecoveryKey, RecoveryAttempts>,
    pending: Option<ToolRecoveryDiagnostic>,
    pending_kind: Option<RecoveryKind>,
    last_inference_fingerprint: Option<String>,
    active_plan_defects: BTreeMap<String, BTreeMap<String, String>>,
    plan_defect_progress: Option<PlanDefectProgress>,
}

impl RecoveryCoordinator {
    /// Advances only for a material plan revision or a new useful tool result.
    /// Model-authored prose and diagnostic wording are intentionally absent.
    pub(super) fn observe_checkpoint(&mut self, checkpoint: RecoveryCheckpoint) {
        if checkpoint == self.checkpoint {
            return;
        }
        self.checkpoint = checkpoint;
        self.attempts.clear();
        self.pending = None;
        self.pending_kind = None;
        self.last_inference_fingerprint = None;
        self.active_plan_defects.clear();
        self.plan_defect_progress = None;
    }

    /// Acceptance resolves only plan-validation guidance. An unchanged plan
    /// is not a new checkpoint and cannot replenish any retry allowance.
    pub(super) fn resolve_plan_validation(&mut self) {
        if self.pending_kind == Some(RecoveryKind::PlanValidation) {
            self.pending = None;
            self.pending_kind = None;
        }
        self.active_plan_defects.clear();
        self.plan_defect_progress = None;
    }

    pub(super) fn pending(&self) -> Option<ToolRecoveryDiagnostic> {
        self.pending.clone()
    }

    pub(super) const fn plan_defect_progress(&self) -> Option<PlanDefectProgress> {
        self.plan_defect_progress
    }

    /// Registers a bounded repair at this semantic checkpoint. Distinct inputs
    /// remain visible for diagnostics, but an unchanged model call is allowed to
    /// receive the same actionable correction more than once before termination.
    pub(super) fn request(
        &mut self,
        kind: RecoveryKind,
        scope: impl Into<String>,
        input_fingerprint: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> RecoveryDecision {
        let key = RecoveryKey {
            kind,
            scope: scope.into(),
        };
        let attempts = self.attempts.entry(key).or_default();
        let input_fingerprint = input_fingerprint.into();
        if attempts.total >= MAX_RECOVERY_ATTEMPTS {
            return RecoveryDecision::Terminal(exhausted_recovery_error(kind));
        }
        attempts.inputs.insert(input_fingerprint);
        attempts.total = attempts.total.saturating_add(1);
        let attempt = attempts.total;
        self.pending = Some(ToolRecoveryDiagnostic::new(
            kind.diagnostic_name().to_owned(),
            diagnostic.into(),
            attempt,
            MAX_RECOVERY_ATTEMPTS,
        ));
        self.pending_kind = Some(kind);
        RecoveryDecision::Resume
    }

    /// Rejects an unchanged recovery inference before paying provider latency.
    /// Typed provider transience is the only identical-input exception because
    /// its changing condition exists outside the prompt and tool state.
    pub(super) fn admit_inference(&mut self, fingerprint: String) -> RecoveryDecision {
        let identical = self
            .last_inference_fingerprint
            .as_ref()
            .is_some_and(|prior| prior == &fingerprint);
        if identical && self.pending_kind != Some(RecoveryKind::TransientProvider) {
            return RecoveryDecision::Terminal(repeated_no_progress_error(
                self.pending_kind.unwrap_or(RecoveryKind::OutputExhaustion),
            ));
        }
        self.last_inference_fingerprint = Some(fingerprint);
        RecoveryDecision::Resume
    }

    /// Registers an externally transient retry. Equal operation arguments are
    /// safe here because the changing typed input is the provider/tool failure
    /// epoch, not model-authored prompt wording.
    pub(super) fn request_transient(
        &mut self,
        kind: RecoveryKind,
        scope: impl Into<String>,
        operation_fingerprint: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> RecoveryDecision {
        debug_assert!(matches!(
            kind,
            RecoveryKind::TransientProvider | RecoveryKind::TransientTool
        ));
        let scope = scope.into();
        let key = RecoveryKey {
            kind,
            scope: scope.clone(),
        };
        let epoch = self
            .attempts
            .get(&key)
            .map_or(1, |attempts| attempts.total.saturating_add(1));
        self.request(
            kind,
            scope,
            format!("{}:transient_epoch:{epoch}", operation_fingerprint.into()),
            diagnostic,
        )
    }

    /// Measures plan repair against the reported defect identities and their
    /// received values. Removing defects or changing an existing defect's
    /// offending value is progress; unrelated plan edits and newly introduced
    /// defects do not independently earn progress. A newly visible defect may
    /// accompany progress when repairing a coarse structural defect lets deeper
    /// validation inspect more of the plan.
    pub(super) fn request_plan_defects(
        &mut self,
        scope: impl Into<String>,
        defects: Vec<(String, String)>,
        diagnostic: impl Into<String>,
    ) -> RecoveryDecision {
        let scope = scope.into();
        let current = defects.into_iter().collect::<BTreeMap<_, _>>();
        if let Some(previous) = self.active_plan_defects.get(&scope) {
            let removed = previous
                .keys()
                .filter(|identity| !current.contains_key(*identity))
                .count();
            let changed = current
                .iter()
                .filter(|(identity, projection)| {
                    previous
                        .get(*identity)
                        .is_some_and(|prior| prior != *projection)
                })
                .count();
            let newly_visible = current
                .keys()
                .filter(|identity| !previous.contains_key(*identity))
                .count();
            self.plan_defect_progress = Some(PlanDefectProgress {
                removed,
                changed,
                newly_visible,
            });
            if removed == 0 && changed == 0 {
                return RecoveryDecision::Terminal(repeated_no_progress_error(
                    RecoveryKind::PlanValidation,
                ));
            }
        } else {
            self.plan_defect_progress = Some(PlanDefectProgress {
                newly_visible: current.len(),
                ..PlanDefectProgress::default()
            });
        }
        for (identity, projection) in &current {
            let key = RecoveryKey {
                kind: RecoveryKind::PlanValidation,
                scope: format!("{scope}:defect:{identity}"),
            };
            let attempts = self.attempts.entry(key).or_default();
            if !attempts.inputs.contains(projection)
                && attempts.inputs.len() >= MAX_RECOVERY_ATTEMPTS
            {
                return RecoveryDecision::Terminal(exhausted_recovery_error(
                    RecoveryKind::PlanValidation,
                ));
            }
        }
        let mut attempt = 1;
        for (identity, projection) in &current {
            let key = RecoveryKey {
                kind: RecoveryKind::PlanValidation,
                scope: format!("{scope}:defect:{identity}"),
            };
            let attempts = self.attempts.entry(key).or_default();
            attempts.inputs.insert(projection.clone());
            attempt = attempt.max(attempts.inputs.len());
        }
        self.active_plan_defects.insert(scope, current);
        self.pending = Some(ToolRecoveryDiagnostic::new(
            RecoveryKind::PlanValidation.diagnostic_name().to_owned(),
            diagnostic.into(),
            attempt,
            MAX_RECOVERY_ATTEMPTS,
        ));
        self.pending_kind = Some(RecoveryKind::PlanValidation);
        RecoveryDecision::Resume
    }
}

fn repeated_no_progress_error(kind: RecoveryKind) -> IpcError {
    IpcError {
        code: IpcErrorCode::Unsupported,
        message: format!(
            "Minutus stopped recovery after the same {} input repeated without progress. Continue by changing the operation arguments or scope, revising the plan from new evidence, or clarifying the requested outcome.",
            kind.diagnostic_name()
        ),
        retryable: true,
    }
}

fn exhausted_recovery_error(kind: RecoveryKind) -> IpcError {
    let continuation = match kind {
        RecoveryKind::TransientProvider => {
            "Continue after the local model service is responsive, or select another installed model."
        }
        RecoveryKind::TransientTool => {
            "Continue after the workspace operation is available, or revise the plan to use a different supported operation."
        }
        _ => {
            "Continue with materially changed arguments, new useful evidence, a revised plan, or a clarifying user instruction."
        }
    };
    IpcError {
        code: IpcErrorCode::Unsupported,
        message: format!(
            "Minutus exhausted three bounded {} recovery attempts. {continuation}",
            kind.diagnostic_name()
        ),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolving_plan_guidance_does_not_replenish_retry_allowance() {
        let mut recovery = RecoveryCoordinator::default();
        for _ in 0..MAX_RECOVERY_ATTEMPTS {
            assert!(matches!(
                recovery.request(RecoveryKind::PlanValidation, "initial", "same", "repair"),
                RecoveryDecision::Resume
            ));
            recovery.resolve_plan_validation();
            assert!(recovery.pending().is_none());
        }
        assert!(matches!(
            recovery.request(RecoveryKind::PlanValidation, "initial", "same", "repair"),
            RecoveryDecision::Terminal(_)
        ));
    }

    #[test]
    fn repeated_recoverable_input_receives_three_bounded_corrections() {
        let mut recovery = RecoveryCoordinator::default();
        recovery.observe_checkpoint(RecoveryCheckpoint {
            plan_revision: 1,
            forecast_revision: 0,
            useful_result_sequence: 2,
        });
        assert!(matches!(
            recovery.request(
                RecoveryKind::InvalidArguments,
                "step:write",
                "args:a",
                "Correct the advertised schema."
            ),
            RecoveryDecision::Resume
        ));
        for _ in 1..MAX_RECOVERY_ATTEMPTS {
            assert!(matches!(
                recovery.request(
                    RecoveryKind::InvalidArguments,
                    "step:write",
                    "args:a",
                    "Repeat the same actionable correction."
                ),
                RecoveryDecision::Resume
            ));
        }
        assert!(matches!(
            recovery.request(
                RecoveryKind::InvalidArguments,
                "step:write",
                "args:a",
                "The bounded retry account is now exhausted."
            ),
            RecoveryDecision::Terminal(_)
        ));
    }

    #[test]
    fn three_changed_inputs_are_allowed_and_new_evidence_starts_a_checkpoint() {
        let mut recovery = RecoveryCoordinator::default();
        for input in ["args:a", "args:b", "args:c"] {
            assert!(matches!(
                recovery.request(
                    RecoveryKind::PlanValidation,
                    "step:evidence",
                    input,
                    "Repair only this step."
                ),
                RecoveryDecision::Resume
            ));
        }
        assert!(matches!(
            recovery.request(
                RecoveryKind::PlanValidation,
                "step:evidence",
                "args:d",
                "Repair only this step."
            ),
            RecoveryDecision::Terminal(_)
        ));
        recovery.observe_checkpoint(RecoveryCheckpoint {
            plan_revision: 0,
            forecast_revision: 0,
            useful_result_sequence: 1,
        });
        assert!(matches!(
            recovery.request(
                RecoveryKind::PlanValidation,
                "step:evidence",
                "args:d",
                "Repair only this step."
            ),
            RecoveryDecision::Resume
        ));
    }

    #[test]
    fn prompt_rewording_does_not_reset_attempts() {
        let mut recovery = RecoveryCoordinator::default();
        let _ = recovery.request(
            RecoveryKind::UnavailableTool,
            "step:convert",
            "registry:v1",
            "Use an advertised tool.",
        );
        assert!(matches!(
            recovery.request(
                RecoveryKind::UnavailableTool,
                "step:convert",
                "registry:v1",
                "Please choose one of the advertised tools instead."
            ),
            RecoveryDecision::Resume
        ));
        assert!(matches!(
            recovery.request(
                RecoveryKind::UnavailableTool,
                "step:convert",
                "registry:v1",
                "Use the current registry."
            ),
            RecoveryDecision::Resume
        ));
        assert!(matches!(
            recovery.request(
                RecoveryKind::UnavailableTool,
                "step:convert",
                "registry:v1",
                "Rewording cannot create a fourth attempt."
            ),
            RecoveryDecision::Terminal(_)
        ));
    }

    #[test]
    fn plan_defect_recovery_rewards_shrinking_sets_not_unrelated_edits() {
        let mut recovery = RecoveryCoordinator::default();
        assert!(matches!(
            recovery.request_plan_defects(
                "plan:0",
                vec![
                    ("write:path".to_owned(), "bad-write".to_owned()),
                    ("compute:path".to_owned(), "bad-compute".to_owned())
                ],
                "Repair both objects."
            ),
            RecoveryDecision::Resume
        ));
        assert!(matches!(
            recovery.request_plan_defects(
                "plan:0",
                vec![("write:path".to_owned(), "bad-write".to_owned())],
                "One defect was removed."
            ),
            RecoveryDecision::Resume
        ));
        assert!(matches!(
            recovery.request_plan_defects(
                "plan:0",
                vec![("write:path".to_owned(), "bad-write".to_owned())],
                "Only unrelated plan text changed."
            ),
            RecoveryDecision::Terminal(_)
        ));
        assert_eq!(
            recovery.plan_defect_progress(),
            Some(PlanDefectProgress {
                removed: 0,
                changed: 0,
                newly_visible: 0,
            })
        );
    }

    #[test]
    fn introduced_only_plan_defects_do_not_earn_recovery() {
        let mut recovery = RecoveryCoordinator::default();
        let _ = recovery.request_plan_defects(
            "plan:0",
            vec![("write:path".to_owned(), "bad-write".to_owned())],
            "Repair the object.",
        );
        assert!(matches!(
            recovery.request_plan_defects(
                "plan:0",
                vec![
                    ("write:path".to_owned(), "bad-write".to_owned()),
                    ("new:path".to_owned(), "new-defect".to_owned())
                ],
                "A new defect is not progress."
            ),
            RecoveryDecision::Terminal(_)
        ));
        assert_eq!(
            recovery.plan_defect_progress(),
            Some(PlanDefectProgress {
                removed: 0,
                changed: 0,
                newly_visible: 1,
            })
        );
    }

    #[test]
    fn newly_visible_plan_defect_does_not_cancel_demonstrated_progress() {
        let mut recovery = RecoveryCoordinator::default();
        assert!(matches!(
            recovery.request_plan_defects(
                "plan:0",
                vec![(
                    "semantic_execution_v6:invalid_array:plan:/steps".to_owned(),
                    "string:994".to_owned()
                )],
                "Use a native steps array."
            ),
            RecoveryDecision::Resume
        ));
        assert_eq!(
            recovery.plan_defect_progress(),
            Some(PlanDefectProgress {
                removed: 0,
                changed: 0,
                newly_visible: 1,
            })
        );
        assert!(matches!(
            recovery.request_plan_defects(
                "plan:0",
                vec![(
                    "semantic_execution_v6:invalid_suggested_tool:plan:/steps/2/suggestedTools/0"
                        .to_owned(),
                    "filesystem.inspect_image".to_owned()
                )],
                "Move image inspection to a read stage."
            ),
            RecoveryDecision::Resume
        ));
        assert_eq!(
            recovery.plan_defect_progress(),
            Some(PlanDefectProgress {
                removed: 1,
                changed: 0,
                newly_visible: 1,
            })
        );
    }

    #[test]
    fn changed_defect_can_progress_while_deeper_validation_reveals_another() {
        let mut recovery = RecoveryCoordinator::default();
        let _ = recovery.request_plan_defects(
            "plan:0",
            vec![("write:path".to_owned(), "bad-write".to_owned())],
            "Repair the write object.",
        );
        assert!(matches!(
            recovery.request_plan_defects(
                "plan:0",
                vec![
                    ("write:path".to_owned(), "better-write".to_owned()),
                    ("new:path".to_owned(), "new-defect".to_owned())
                ],
                "The write changed and deeper validation found another defect."
            ),
            RecoveryDecision::Resume
        ));
    }

    #[test]
    fn every_historical_repairable_host_class_survives_its_first_diagnostic() {
        for kind in [
            RecoveryKind::InvalidArguments,
            RecoveryKind::StalePhase,
            RecoveryKind::UnavailableTool,
            RecoveryKind::PlanValidation,
            RecoveryKind::OutputExhaustion,
            RecoveryKind::TransientProvider,
            RecoveryKind::TransientTool,
        ] {
            let mut recovery = RecoveryCoordinator::default();
            assert!(matches!(
                recovery.request(kind, "current-step", "typed-change", "Repair this step."),
                RecoveryDecision::Resume
            ));
            assert!(recovery.pending().is_some());
        }
    }

    #[test]
    fn transient_external_state_is_bounded_without_fabricating_argument_changes() {
        let mut recovery = RecoveryCoordinator::default();
        for _ in 0..MAX_RECOVERY_ATTEMPTS {
            assert!(matches!(
                recovery.request_transient(
                    RecoveryKind::TransientProvider,
                    "current-step",
                    "same-provider-input",
                    "Retry after typed provider transience."
                ),
                RecoveryDecision::Resume
            ));
        }
        assert!(matches!(
            recovery.request_transient(
                RecoveryKind::TransientProvider,
                "current-step",
                "same-provider-input",
                "Retry after typed provider transience."
            ),
            RecoveryDecision::Terminal(_)
        ));
    }

    #[test]
    fn identical_non_provider_recovery_input_is_suppressed_before_inference() {
        let mut recovery = RecoveryCoordinator::default();
        assert!(matches!(
            recovery.admit_inference("rich:evidence:tools:profile".to_owned()),
            RecoveryDecision::Resume
        ));
        let _ = recovery.request(
            RecoveryKind::OutputExhaustion,
            "current-step",
            "mode:constrained",
            "Emit the outstanding call.",
        );
        assert!(matches!(
            recovery.admit_inference("rich:evidence:tools:profile".to_owned()),
            RecoveryDecision::Terminal(_)
        ));
    }
}
