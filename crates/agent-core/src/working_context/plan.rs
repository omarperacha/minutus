use std::{collections::BTreeSet, fmt, ops::Range};

const MAX_ROUNDS: usize = 1_024;

/// Trusted limits for one derived inference-history view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkingContextPolicy {
    max_history: u64,
    recent_turn: u64,
    summary: u64,
}

impl WorkingContextPolicy {
    /// Creates a policy whose recent and summary budgets fit the total history budget.
    ///
    /// # Errors
    ///
    /// Returns an error for zero or internally inconsistent limits.
    pub fn new(
        max_history_tokens: u64,
        recent_turn_tokens: u64,
        summary_tokens: u64,
    ) -> Result<Self, WorkingContextError> {
        if max_history_tokens == 0
            || recent_turn_tokens == 0
            || summary_tokens == 0
            || recent_turn_tokens
                .checked_add(summary_tokens)
                .is_none_or(|total| total > max_history_tokens)
        {
            return Err(WorkingContextError::InvalidPolicy);
        }
        Ok(Self {
            max_history: max_history_tokens,
            recent_turn: recent_turn_tokens,
            summary: summary_tokens,
        })
    }

    /// Maximum trusted estimate for the compact recap.
    #[must_use]
    pub const fn summary_tokens(self) -> u64 {
        self.summary
    }
}

/// One indivisible user turn, including any completed tool round it initiated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkingContextRound {
    id: String,
    message_range: Range<usize>,
    token_estimate: u64,
}

impl WorkingContextRound {
    /// Creates one bounded round selection unit.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid ID, empty message range or zero estimate.
    pub fn new(
        id: impl Into<String>,
        message_range: Range<usize>,
        token_estimate: u64,
    ) -> Result<Self, WorkingContextError> {
        let id = id.into();
        if id.is_empty()
            || id.len() > 160
            || id.chars().any(char::is_whitespace)
            || message_range.is_empty()
            || token_estimate == 0
        {
            return Err(WorkingContextError::InvalidRound);
        }
        Ok(Self {
            id,
            message_range,
            token_estimate,
        })
    }

    /// Stable ID of the first message in the round.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Complete message range retained or compacted as one unit.
    #[must_use]
    pub fn message_range(&self) -> Range<usize> {
        self.message_range.clone()
    }
}

/// Deterministic selection of compacted and verbatim history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkingContextPlan {
    compacted_rounds: Range<usize>,
    recent_rounds: Range<usize>,
    recent_tokens: u64,
    summary_token_budget: u64,
}

impl WorkingContextPlan {
    /// Whether any older rounds require a recap.
    #[must_use]
    pub fn is_compacted(&self) -> bool {
        !self.compacted_rounds.is_empty()
    }

    /// Older complete rounds replaced by the untrusted recap.
    #[must_use]
    pub fn compacted_rounds(&self) -> Range<usize> {
        self.compacted_rounds.clone()
    }

    /// Recent complete rounds retained verbatim.
    #[must_use]
    pub fn recent_rounds(&self) -> Range<usize> {
        self.recent_rounds.clone()
    }

    /// Trusted estimate consumed by recent verbatim rounds.
    #[must_use]
    pub const fn recent_tokens(&self) -> u64 {
        self.recent_tokens
    }

    /// Maximum trusted estimate available to the recap.
    #[must_use]
    pub const fn summary_token_budget(&self) -> u64 {
        self.summary_token_budget
    }
}

/// Host-owned deterministic working-context selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkingContextCompactor {
    policy: WorkingContextPolicy,
}

impl WorkingContextCompactor {
    /// Creates a selector using trusted host policy.
    #[must_use]
    pub const fn new(policy: WorkingContextPolicy) -> Self {
        Self { policy }
    }

    /// Selects a complete recent suffix and marks all older rounds for compaction.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, overlapping or excessive rounds, checked
    /// arithmetic overflow, or a newest round that cannot fit verbatim.
    pub fn compact(
        self,
        rounds: &[WorkingContextRound],
    ) -> Result<WorkingContextPlan, WorkingContextError> {
        validate_rounds(rounds)?;
        let total = rounds.iter().try_fold(0_u64, |sum, round| {
            sum.checked_add(round.token_estimate)
                .ok_or(WorkingContextError::EstimateOverflow)
        })?;
        if total <= self.policy.max_history {
            return Ok(WorkingContextPlan {
                compacted_rounds: 0..0,
                recent_rounds: 0..rounds.len(),
                recent_tokens: total,
                summary_token_budget: 0,
            });
        }

        let mut recent_tokens = 0_u64;
        let mut recent_start = rounds.len();
        for (index, round) in rounds.iter().enumerate().rev() {
            let candidate = recent_tokens
                .checked_add(round.token_estimate)
                .ok_or(WorkingContextError::EstimateOverflow)?;
            if candidate > self.policy.recent_turn {
                break;
            }
            recent_tokens = candidate;
            recent_start = index;
        }
        if recent_start == rounds.len() {
            return Err(WorkingContextError::RecentRoundTooLarge {
                required: rounds.last().map_or(0, |round| round.token_estimate),
                available: self.policy.recent_turn,
            });
        }
        Ok(WorkingContextPlan {
            compacted_rounds: 0..recent_start,
            recent_rounds: recent_start..rounds.len(),
            recent_tokens,
            summary_token_budget: self.policy.summary,
        })
    }
}

fn validate_rounds(rounds: &[WorkingContextRound]) -> Result<(), WorkingContextError> {
    if rounds.is_empty() || rounds.len() > MAX_ROUNDS {
        return Err(WorkingContextError::InvalidRound);
    }
    let mut ids = BTreeSet::new();
    let mut expected_start = 0;
    for round in rounds {
        if !ids.insert(round.id.as_str()) || round.message_range.start != expected_start {
            return Err(WorkingContextError::InvalidRound);
        }
        expected_start = round.message_range.end;
    }
    Ok(())
}

/// Recoverable working-context construction failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkingContextError {
    /// Trusted limits are empty or inconsistent.
    InvalidPolicy,
    /// A round is malformed, duplicated, discontinuous or excessive.
    InvalidRound,
    /// Checked token accounting overflowed.
    EstimateOverflow,
    /// The newest complete turn cannot fit the explicit verbatim budget.
    RecentRoundTooLarge {
        /// Trusted estimate required by the newest round.
        required: u64,
        /// Configured recent-turn allowance.
        available: u64,
    },
    /// Mandatory current context cannot fit without splitting an active round.
    ContextCapacityGap,
    /// A recap is empty, malformed or exceeds its bound.
    InvalidSummary,
    /// A persisted semantic checkpoint is malformed, stale or unsupported.
    InvalidCheckpoint,
}

impl fmt::Display for WorkingContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy => formatter.write_str("working-context policy is invalid"),
            Self::InvalidRound => formatter.write_str("working-context round is invalid"),
            Self::EstimateOverflow => {
                formatter.write_str("working-context token estimate overflowed")
            }
            Self::RecentRoundTooLarge {
                required,
                available,
            } => write!(
                formatter,
                "recent turn requires {required} tokens but only {available} are available"
            ),
            Self::ContextCapacityGap => formatter.write_str("context capacity gap: required current inputs cannot fit safely; use a model with a larger context or narrow the current input while retaining this conversation for exact recall"),
            Self::InvalidSummary => formatter.write_str("working-context summary is invalid"),
            Self::InvalidCheckpoint => {
                formatter.write_str("semantic compaction checkpoint is invalid")
            }
        }
    }
}

impl std::error::Error for WorkingContextError {}
