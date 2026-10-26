//! Versioned model-neutral reasoning and output policy for one execution round.

/// Version of the execution-profile selection and budget policy.
pub const EXECUTION_PROFILE_POLICY_VERSION: u16 = 6;

/// Host-observed unfinished work for the next model round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionWorkKind {
    /// Establish or revise a provisional execution forecast.
    Planning,
    /// Inspect or extract source evidence.
    Evidence,
    /// Transform an established representation or run bounded computation.
    Transformation,
    /// Perform a forecasted outcome-bearing mutation.
    Mutation,
    /// Compose the ordinary user-visible response from established facts.
    FinalResponse,
    /// Perform one bounded deterministic correction or continuation.
    Recovery,
}

impl ExecutionWorkKind {
    /// Stable content-free diagnostic name.
    #[must_use]
    pub const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Evidence => "evidence",
            Self::Transformation => "transformation",
            Self::Mutation => "mutation",
            Self::FinalResponse => "final_response",
            Self::Recovery => "recovery",
        }
    }
}

/// Host-observed risk that may justify more reasoning capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionRisk {
    /// The expected operation is routine and reversible or read-only.
    Routine,
    /// Ambiguous evidence or prior failures make the next decision less certain.
    Ambiguous,
    /// Incorrect execution could apply an outcome-bearing effect.
    OutcomeBearing,
}

/// Bounded measurements available before prompt rendering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionComplexity {
    /// Complete conversation messages admitted to working-context derivation.
    pub message_count: usize,
    /// Rich inputs still directly visible to the round.
    pub image_count: usize,
    /// Whether directly visible rich input came from the one explicit bounded
    /// reinspection permitted for the current semantic stage.
    pub rich_input_reinspection: bool,
    /// Forecast calls remaining in the current accepted plan.
    pub unfinished_calls: usize,
    /// Host-recorded corrections and plan revisions already consumed.
    pub deviations: usize,
}

/// Complete trusted inputs to the profile selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionProfileSignals {
    /// Type of unfinished work.
    pub work: ExecutionWorkKind,
    /// Risk of the next decision.
    pub risk: ExecutionRisk,
    /// Measured bounded complexity.
    pub complexity: ExecutionComplexity,
}

/// Model-neutral bounded execution profiles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionProfile {
    /// Prefer low latency for routine and deterministic continuation work.
    Latency,
    /// Retain moderate capacity for ordinary planning and transformations.
    Balanced,
    /// Retain the configured provider ceiling for genuinely difficult work.
    Deep,
}

impl ExecutionProfile {
    /// Stable content-free diagnostic name.
    #[must_use]
    pub const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Latency => "latency",
            Self::Balanced => "balanced",
            Self::Deep => "deep",
        }
    }

    /// Reasoning reserve within one configured provider output ceiling.
    #[must_use]
    pub fn reasoning_reserve(self, maximum_output_tokens: u32) -> u64 {
        let divisor = match self {
            Self::Latency => 16,
            Self::Balanced => 8,
            Self::Deep => 4,
        };
        u64::from(maximum_output_tokens / divisor).max(1)
    }

    /// Bounded generation ceiling before final-answer preservation is applied.
    #[must_use]
    pub fn output_limit(self, maximum_output_tokens: u32) -> u32 {
        let divisor = match self {
            Self::Latency => 4,
            Self::Balanced => 2,
            Self::Deep => 1,
        };
        (maximum_output_tokens / divisor)
            .max(2)
            .min(maximum_output_tokens)
    }
}

/// Versioned profile selected entirely from host-owned signals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionProfileDecision {
    /// Selected bounded profile.
    pub profile: ExecutionProfile,
    /// Work classification that produced the decision.
    pub work: ExecutionWorkKind,
    /// Policy version used for evaluation attribution.
    pub policy_version: u16,
    /// Whether a host-proven bounded action needs additional structured-output
    /// room without increasing private reasoning strength.
    pub bounded_action: bool,
    /// An available tool can carry a whole artifact; reserve payload capacity
    /// independently from reasoning strength and the advisory stage cursor.
    pub bulk_arguments: bool,
}

impl ExecutionProfileDecision {
    /// Selects one profile without accepting model-authored limits.
    #[must_use]
    pub fn select(signals: ExecutionProfileSignals) -> Self {
        let non_media_complex = signals.complexity.unfinished_calls >= 6
            || signals.complexity.message_count >= 24
            || signals.complexity.deviations >= 2;
        let complex = signals.complexity.image_count > 0 || non_media_complex;
        // Reinspection is a transport fact, not evidence of semantic ambiguity.
        let deep = (signals.work == ExecutionWorkKind::Evidence
            && signals.risk == ExecutionRisk::Ambiguous)
            || (signals.work == ExecutionWorkKind::Planning && non_media_complex)
            || (signals.work != ExecutionWorkKind::Recovery
                && signals.risk == ExecutionRisk::Ambiguous
                && complex);
        let profile = if deep {
            ExecutionProfile::Deep
        } else {
            match (signals.work, signals.risk, complex) {
                // A write-stage cursor does not mean visual interpretation has
                // finished. Retained source pixels still need normal capacity.
                (ExecutionWorkKind::Mutation, _, _) if signals.complexity.image_count > 0 => {
                    ExecutionProfile::Balanced
                }
                (ExecutionWorkKind::Mutation | ExecutionWorkKind::FinalResponse, _, _)
                | (ExecutionWorkKind::Recovery, ExecutionRisk::Routine, _) => {
                    ExecutionProfile::Latency
                }
                (ExecutionWorkKind::Recovery, ExecutionRisk::Ambiguous, _) => {
                    ExecutionProfile::Balanced
                }
                (ExecutionWorkKind::Planning | ExecutionWorkKind::Transformation, _, _)
                | (_, ExecutionRisk::OutcomeBearing, _)
                | (_, _, true) => ExecutionProfile::Balanced,
                _ => ExecutionProfile::Latency,
            }
        };
        Self {
            profile,
            work: signals.work,
            policy_version: EXECUTION_PROFILE_POLICY_VERSION,
            bounded_action: false,
            bulk_arguments: false,
        }
    }

    /// Selects concise reasoning while retaining a larger structured-action
    /// output allowance for one host-proven bounded stage.
    #[must_use]
    pub const fn for_bounded_action(mut self) -> Self {
        self.profile = ExecutionProfile::Latency;
        self.bounded_action = true;
        self
    }

    /// Whether this round may directly produce the ordinary final answer.
    #[must_use]
    pub const fn preserves_final_answer_output(self) -> bool {
        matches!(self.work, ExecutionWorkKind::FinalResponse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(work: ExecutionWorkKind) -> ExecutionProfileSignals {
        ExecutionProfileSignals {
            work,
            risk: ExecutionRisk::Routine,
            complexity: ExecutionComplexity {
                message_count: 4,
                image_count: 0,
                rich_input_reinspection: false,
                unfinished_calls: 1,
                deviations: 0,
            },
        }
    }

    #[test]
    fn routine_mutation_recovery_and_final_response_prefer_latency() {
        for work in [
            ExecutionWorkKind::Mutation,
            ExecutionWorkKind::Recovery,
            ExecutionWorkKind::FinalResponse,
        ] {
            assert_eq!(
                ExecutionProfileDecision::select(signals(work)).profile,
                ExecutionProfile::Latency
            );
        }
        let bounded = ExecutionProfileDecision::select(signals(ExecutionWorkKind::Transformation))
            .for_bounded_action();
        assert_eq!(bounded.profile, ExecutionProfile::Latency);
        assert!(bounded.bounded_action);
    }

    #[test]
    fn ambiguity_and_measured_complexity_retain_capacity() {
        let mut evidence = signals(ExecutionWorkKind::Evidence);
        evidence.risk = ExecutionRisk::Ambiguous;
        evidence.complexity.rich_input_reinspection = true;
        assert_eq!(
            ExecutionProfileDecision::select(evidence).profile,
            ExecutionProfile::Deep
        );
        let mut planning = signals(ExecutionWorkKind::Planning);
        planning.complexity.unfinished_calls = 8;
        assert_eq!(
            ExecutionProfileDecision::select(planning).profile,
            ExecutionProfile::Deep
        );

        let mut mutation = signals(ExecutionWorkKind::Mutation);
        mutation.risk = ExecutionRisk::OutcomeBearing;
        mutation.complexity.image_count = 1;
        assert_eq!(
            ExecutionProfileDecision::select(mutation).profile,
            ExecutionProfile::Balanced
        );
        mutation.complexity.image_count = 0;
        assert_eq!(
            ExecutionProfileDecision::select(mutation).profile,
            ExecutionProfile::Latency
        );
        let mut recovery = signals(ExecutionWorkKind::Recovery);
        recovery.risk = ExecutionRisk::Ambiguous;
        assert_eq!(
            ExecutionProfileDecision::select(recovery).profile,
            ExecutionProfile::Balanced
        );

        let mut corrected_evidence = signals(ExecutionWorkKind::Evidence);
        corrected_evidence.complexity.deviations = 3;
        assert_eq!(
            ExecutionProfileDecision::select(corrected_evidence).profile,
            ExecutionProfile::Balanced,
            "past corrections may increase bounded capacity but must not force deep evidence reasoning"
        );
    }

    #[test]
    fn routine_image_reinspection_keeps_the_initial_balanced_profile() {
        let mut first_image = signals(ExecutionWorkKind::Evidence);
        first_image.complexity.image_count = 1;
        assert_eq!(
            ExecutionProfileDecision::select(first_image).profile,
            ExecutionProfile::Balanced
        );

        let mut reinspection = first_image;
        reinspection.complexity.rich_input_reinspection = true;
        assert_eq!(
            ExecutionProfileDecision::select(reinspection).profile,
            ExecutionProfile::Balanced
        );

        let mut historical_image_only = signals(ExecutionWorkKind::Evidence);
        historical_image_only.complexity.message_count = 8;
        assert_eq!(
            ExecutionProfileDecision::select(historical_image_only).profile,
            ExecutionProfile::Latency
        );

        let mut planning_with_image = signals(ExecutionWorkKind::Planning);
        planning_with_image.complexity.image_count = 1;
        assert_eq!(
            ExecutionProfileDecision::select(planning_with_image).profile,
            ExecutionProfile::Balanced
        );
    }

    #[test]
    fn profile_limits_are_ordered_and_never_raise_the_host_ceiling() {
        let maximum = 8_192;
        assert_eq!(ExecutionProfile::Latency.output_limit(maximum), 2_048);
        assert_eq!(ExecutionProfile::Balanced.output_limit(maximum), 4_096);
        assert_eq!(ExecutionProfile::Deep.output_limit(maximum), maximum);
        for profile in [
            ExecutionProfile::Latency,
            ExecutionProfile::Balanced,
            ExecutionProfile::Deep,
        ] {
            assert!(profile.output_limit(maximum) <= maximum);
            assert!(profile.reasoning_reserve(maximum) < u64::from(maximum));
        }
    }
}
