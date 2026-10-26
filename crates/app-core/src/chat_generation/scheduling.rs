//! Host-owned limits supplied to one desktop tool batch.

use agent_core::ExecutionBudgets;

const DEFAULT_CALLS: usize = 8;
const DEFAULT_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_ELAPSED_MS: u64 = 10_000;
const APPROXIMATE_BYTES_PER_CONTEXT_TOKEN: u64 = 4;

/// Remaining host limits used before scheduling another tool batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolScheduleLimits {
    calls: usize,
    output_bytes: usize,
    elapsed_ms: u64,
}

impl ToolScheduleLimits {
    pub(crate) fn from_budgets(budgets: Option<ExecutionBudgets>) -> Self {
        let Some(budgets) = budgets else {
            return Self {
                calls: DEFAULT_CALLS,
                output_bytes: DEFAULT_OUTPUT_BYTES,
                elapsed_ms: DEFAULT_ELAPSED_MS,
            };
        };
        let context_output_bytes = budgets
            .context_tokens
            .saturating_sub(budgets.final_response_tokens)
            .saturating_mul(APPROXIMATE_BYTES_PER_CONTEXT_TOKEN);
        Self {
            calls: usize::try_from(budgets.tool_calls)
                .unwrap_or(usize::MAX)
                .min(DEFAULT_CALLS),
            output_bytes: usize::try_from(budgets.tool_output_bytes.min(context_output_bytes))
                .unwrap_or(usize::MAX)
                .min(DEFAULT_OUTPUT_BYTES),
            elapsed_ms: budgets
                .tool_ms
                .min(budgets.elapsed_ms)
                .min(DEFAULT_ELAPSED_MS),
        }
    }

    /// Maximum new effects admitted in this batch.
    #[must_use]
    pub const fn max_calls(self) -> usize {
        self.calls
    }

    /// Maximum serialized model-visible output retained by this batch.
    #[must_use]
    pub const fn max_output_bytes(self) -> usize {
        self.output_bytes
    }

    /// Maximum batch wall time remaining under ledger and product policy.
    #[must_use]
    pub const fn max_elapsed_ms(self) -> u64 {
        self.elapsed_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_final_answer_reserve_reduces_schedulable_output() {
        let limits = ToolScheduleLimits::from_budgets(Some(ExecutionBudgets {
            rounds: 1,
            tool_calls: 3,
            elapsed_ms: 2_000,
            tool_ms: 1_500,
            context_tokens: 1_000,
            final_response_tokens: 900,
            tool_output_bytes: 10_000,
        }));
        assert_eq!(limits.max_calls(), 3);
        assert_eq!(limits.max_output_bytes(), 400);
        assert_eq!(limits.max_elapsed_ms(), 1_500);
    }
}
