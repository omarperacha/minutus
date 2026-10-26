//! Select established work for execution and the final-answer handoff.

use std::collections::{BTreeMap, BTreeSet};

use super::{ToolPlanProgress, ToolPlanStepStatus};

impl ToolPlanProgress {
    pub(super) fn dependency_call_ids(&self) -> BTreeSet<String> {
        let current = self
            .suggested_step
            .and_then(|ordinal| self.steps.get(ordinal.saturating_sub(1)));
        let mut call_ids = current
            .into_iter()
            .flat_map(|step| step.completed_call_ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        if let Some(current) = current {
            let by_id = self
                .steps
                .iter()
                .filter_map(|step| step.id.as_ref().map(|id| (id.as_str(), step)))
                .collect::<BTreeMap<_, _>>();
            let mut pending = current.depends_on.clone();
            let mut dependency_ids = BTreeSet::new();
            while let Some(dependency_id) = pending.pop() {
                if !dependency_ids.insert(dependency_id.clone()) {
                    continue;
                }
                if let Some(dependency) = by_id.get(dependency_id.as_str()) {
                    call_ids.extend(dependency.completed_call_ids.iter().cloned());
                    pending.extend(dependency.depends_on.iter().cloned());
                }
            }
        }
        // With every stage closed, the answer consumes the plan's established
        // results, not an empty dependency frontier. This selects tool pairs;
        // it does not replay private reasoning or change stage/tool admission.
        let answer_handoff = self.suggested_step.is_none()
            && self
                .steps
                .iter()
                .all(|step| step.status == ToolPlanStepStatus::ForecastComplete);
        let ready_step_ids = self.ready_step_ids().into_iter().collect::<BTreeSet<_>>();
        call_ids.extend(
            self.steps
                .iter()
                .filter(|step| {
                    answer_handoff
                        || step
                            .id
                            .as_ref()
                            .is_some_and(|id| ready_step_ids.contains(id))
                })
                .flat_map(|step| step.completed_call_ids.iter().cloned()),
        );
        call_ids.extend(self.retained_call_ids.iter().cloned());
        call_ids
    }

    pub(super) fn ready_step_ids(&self) -> Vec<String> {
        let completed = self
            .steps
            .iter()
            .filter(|step| step.status == ToolPlanStepStatus::ForecastComplete)
            .filter_map(|step| step.id.clone())
            .collect::<BTreeSet<_>>();
        self.steps
            .iter()
            .filter(|step| {
                step.status != ToolPlanStepStatus::ForecastComplete
                    && step
                        .depends_on
                        .iter()
                        .all(|dependency| completed.contains(dependency))
            })
            .filter_map(|step| step.id.clone())
            .collect()
    }
}

impl super::ToolOrchestrationContext {
    pub(crate) fn extend_final_history(
        &self,
        conversation: &ipc_types::Conversation,
        focus: &mut Option<BTreeSet<String>>,
    ) {
        if !self.phase_directive.as_ref().is_some_and(|directive| {
            matches!(directive.scope, super::ToolPhaseScope::FinalResponseReady)
        }) {
            return;
        }
        // Once tools close, no omitted producer may strand a final-answer input.
        // Conservative retention is bounded by exact admission, never truncation
        // of the plan's capped diagnostic call-ID list.
        focus.get_or_insert_with(BTreeSet::new).extend(
            conversation
                .messages
                .iter()
                .flat_map(|message| &message.parts)
                .filter_map(|part| match part {
                    ipc_types::MessagePart::ToolCall { request }
                        if !matches!(
                            request.name,
                            ipc_types::ToolName::OrchestrationSubmitPlan
                                | ipc_types::ToolName::OrchestrationSubmitForecast
                                | ipc_types::ToolName::OrchestrationCompleteStage
                                | ipc_types::ToolName::OrchestrationSearchTools
                        ) =>
                    {
                        Some(request.call_id.0.clone())
                    }
                    _ => None,
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolPlanOutcome, ToolPlanStepProgress};

    fn progress(outcome: ToolPlanOutcome, finished: bool) -> ToolPlanProgress {
        let step = |ordinal, id: &str, dependencies: &[&str], complete, calls: &[&str]| {
            ToolPlanStepProgress::new(
                ordinal,
                if complete {
                    ToolPlanStepStatus::ForecastComplete
                } else {
                    ToolPlanStepStatus::NotStarted
                },
                Vec::new(),
            )
            .with_contract(
                id.to_owned(),
                id.to_owned(),
                Vec::new(),
                "Done".to_owned(),
                calls.iter().map(|id| (*id).to_owned()).collect(),
            )
            .with_dependencies(dependencies.iter().map(|id| (*id).to_owned()).collect())
        };
        ToolPlanProgress::new(
            2,
            1,
            vec![
                step(1, "research", &[], true, &["search", "source"]),
                step(2, "independent", &[], true, &["independent-result"]),
                step(3, "compare", &["research"], finished, &["comparison"]),
            ],
            if finished { None } else { Some(3) },
            outcome,
            outcome == ToolPlanOutcome::CreateFile,
            false,
            3,
            false,
            0,
        )
        .with_retained_call_ids(["unresolved-error".to_owned(), "earlier-revision".to_owned()])
    }

    #[test]
    fn answer_handoff_retains_all_completed_branches_and_existing_recovery_results() {
        for outcome in [
            ToolPlanOutcome::Answer,
            ToolPlanOutcome::Other,
            ToolPlanOutcome::CreateFile,
        ] {
            assert_eq!(
                progress(outcome, true).dependency_call_ids(),
                [
                    "search",
                    "source",
                    "independent-result",
                    "comparison",
                    "unresolved-error",
                    "earlier-revision"
                ]
                .map(str::to_owned)
                .into_iter()
                .collect(),
            );
        }
    }

    #[test]
    fn unfinished_plan_keeps_current_dependency_focus() {
        let progress = progress(ToolPlanOutcome::Answer, false);
        let selected = progress.dependency_call_ids();
        assert!(!selected.contains("independent-result"));
        for id in [
            "search",
            "source",
            "comparison",
            "unresolved-error",
            "earlier-revision",
        ] {
            assert!(selected.contains(id), "missing dependency result {id}");
        }
    }

    #[test]
    fn absent_cursor_does_not_mean_unfinished_stages_are_complete() {
        let mut progress = progress(ToolPlanOutcome::Answer, false);
        progress.suggested_step = None;
        assert!(
            !progress
                .dependency_call_ids()
                .contains("independent-result")
        );
    }
}
