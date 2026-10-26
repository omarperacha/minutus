//! Host-owned context accounting and admission decisions.

use std::{error::Error, fmt};

use crate::RoundAllowance;

/// Token counts for every model-visible input category in one generation.
///
/// Counts are produced by the active model adapter's trusted tokenizer. Keeping
/// the categories separate makes omissions visible while admission still uses
/// one deterministic checked sum.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContextTokenEstimate {
    /// Trusted application system instructions and their rendered framing.
    pub system_instructions: u64,
    /// User and assistant conversation content other than historical calls/results.
    pub conversation_history: u64,
    /// Rendered tool definitions and tool-protocol framing.
    pub tool_definitions: u64,
    /// Historical model-authored tool calls.
    pub tool_calls: u64,
    /// Historical untrusted tool results already present in the prompt.
    pub tool_results: u64,
    /// Visual tokens for every image in prompt order.
    pub images: u64,
}

impl ContextTokenEstimate {
    /// Returns the complete estimated model input, failing closed on overflow.
    ///
    /// # Errors
    ///
    /// Returns [`ContextAdmissionError::ArithmeticOverflow`] if the component
    /// counts cannot be represented by the host accounting type.
    pub fn input_tokens(self) -> Result<u64, ContextAdmissionError> {
        [
            self.system_instructions,
            self.conversation_history,
            self.tool_definitions,
            self.tool_calls,
            self.tool_results,
            self.images,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or(ContextAdmissionError::ArithmeticOverflow)
    }
}

/// Trusted model-specific context policy not duplicated in the execution ledger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextAdmissionPolicy {
    context_window_tokens: u64,
    reasoning_reserve_tokens: u64,
}

impl ContextAdmissionPolicy {
    /// Creates a policy for one verified model profile.
    ///
    /// The final-answer reserve deliberately does not live here: CTX-001 must
    /// consume the remaining host-owned reserve supplied by AGENT-005.
    ///
    /// # Errors
    ///
    /// Returns [`ContextAdmissionError::InvalidPolicy`] for zero values or a
    /// reasoning reserve that alone fills the model context window.
    pub fn new(
        context_window_tokens: u64,
        reasoning_reserve_tokens: u64,
    ) -> Result<Self, ContextAdmissionError> {
        if context_window_tokens == 0
            || reasoning_reserve_tokens == 0
            || reasoning_reserve_tokens >= context_window_tokens
        {
            return Err(ContextAdmissionError::InvalidPolicy);
        }
        Ok(Self {
            context_window_tokens,
            reasoning_reserve_tokens,
        })
    }

    /// Returns the verified model's configured context window.
    #[must_use]
    pub const fn context_window_tokens(self) -> u64 {
        self.context_window_tokens
    }

    /// Returns the tokens explicitly reserved for reasoning each tool round.
    #[must_use]
    pub const fn reasoning_reserve_tokens(self) -> u64 {
        self.reasoning_reserve_tokens
    }
}

/// An admitted tool-capable generation and its exact remaining context capacity.
#[derive(Debug, Eq, PartialEq)]
pub struct AdmittedToolRound {
    estimate: ContextTokenEstimate,
    input_tokens: u64,
    reasoning_reserve_tokens: u64,
    final_response_reserve_tokens: u64,
    available_tool_result_tokens: u64,
    effective_context_limit_tokens: u64,
}

/// Admitted tools-disabled model pass with exact explicit reserves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdmittedModelPass {
    input: u64,
    reasoning_reserve: u64,
    output_reserve: u64,
    effective_context_limit: u64,
}

impl AdmittedModelPass {
    /// Exact tokenizer-derived complete rendered input.
    #[must_use]
    pub const fn input_tokens(self) -> u64 {
        self.input
    }

    /// Explicit reasoning reserve.
    #[must_use]
    pub const fn reasoning_reserve_tokens(self) -> u64 {
        self.reasoning_reserve
    }

    /// Explicit output reserve for this pass.
    #[must_use]
    pub const fn output_reserve_tokens(self) -> u64 {
        self.output_reserve
    }

    /// Effective context limit used for admission.
    #[must_use]
    pub const fn effective_context_limit_tokens(self) -> u64 {
        self.effective_context_limit
    }
}

impl AdmittedToolRound {
    /// Returns the complete categorized input estimate used for the decision.
    #[must_use]
    pub const fn estimate(&self) -> ContextTokenEstimate {
        self.estimate
    }

    /// Returns the summed system, history, tool and image input tokens.
    #[must_use]
    pub const fn input_tokens(&self) -> u64 {
        self.input_tokens
    }

    /// Returns the explicit reasoning capacity held outside model input.
    #[must_use]
    pub const fn reasoning_reserve_tokens(&self) -> u64 {
        self.reasoning_reserve_tokens
    }

    /// Returns the AGENT-005 final-response capacity held outside model input.
    #[must_use]
    pub const fn final_response_reserve_tokens(&self) -> u64 {
        self.final_response_reserve_tokens
    }

    /// Returns the largest additional tool result that can enter the next prompt.
    #[must_use]
    pub const fn available_tool_result_tokens(&self) -> u64 {
        self.available_tool_result_tokens
    }

    /// Returns the smaller of the model window and remaining AGENT-005 context budget.
    #[must_use]
    pub const fn effective_context_limit_tokens(&self) -> u64 {
        self.effective_context_limit_tokens
    }
}

/// Recoverable disposition for one tool result before it enters model context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolResultAdmission {
    /// The original bounded result fits and may be rendered in full.
    Full {
        /// Trusted token estimate for the complete rendered result.
        tokens: u64,
    },
    /// The original result does not fit, but the supplied compact form does.
    Compact {
        /// Trusted token estimate for the compact representation.
        tokens: u64,
    },
}

/// Deterministic host-owned admission gate for a verified model profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextAdmissionController {
    policy: ContextAdmissionPolicy,
}

impl ContextAdmissionController {
    /// Creates an admission gate from validated trusted policy.
    #[must_use]
    pub const fn new(policy: ContextAdmissionPolicy) -> Self {
        Self { policy }
    }

    /// Reserves reasoning and final-answer capacity before a tool round starts.
    ///
    /// The effective limit is capped by `allowance.remaining.context_tokens`, so
    /// the gate consumes AGENT-005's remaining budget rather than maintaining an
    /// independent counter. It never mutates or replenishes ledger state.
    ///
    /// # Errors
    ///
    /// Returns a typed error when tools are disabled, the remaining final-answer
    /// reserve is empty, arithmetic overflows, or the fixed prompt cannot fit.
    pub fn admit_tool_round(
        self,
        allowance: RoundAllowance,
        estimate: ContextTokenEstimate,
    ) -> Result<AdmittedToolRound, ContextAdmissionError> {
        if !allowance.tools_enabled {
            return Err(ContextAdmissionError::ToolsDisabled);
        }
        let final_response_reserve_tokens = allowance.remaining.final_response_tokens;
        if final_response_reserve_tokens == 0 {
            return Err(ContextAdmissionError::FinalResponseReserveExhausted);
        }
        let input_tokens = estimate.input_tokens()?;
        let effective_context_limit_tokens = self
            .policy
            .context_window_tokens
            .min(allowance.remaining.context_tokens);
        let reserved_tokens = input_tokens
            .checked_add(self.policy.reasoning_reserve_tokens)
            .and_then(|tokens| tokens.checked_add(final_response_reserve_tokens))
            .ok_or(ContextAdmissionError::ArithmeticOverflow)?;
        let available_tool_result_tokens = effective_context_limit_tokens
            .checked_sub(reserved_tokens)
            .ok_or(ContextAdmissionError::ContextLimitExceeded {
                required_tokens: reserved_tokens,
                available_tokens: effective_context_limit_tokens,
            })?;
        Ok(AdmittedToolRound {
            estimate,
            input_tokens,
            reasoning_reserve_tokens: self.policy.reasoning_reserve_tokens,
            final_response_reserve_tokens,
            available_tool_result_tokens,
            effective_context_limit_tokens,
        })
    }

    /// Admits a tools-disabled auxiliary model pass without consuming or
    /// replenishing the agent run's final-answer reserve.
    ///
    /// # Errors
    ///
    /// Returns an error for zero output, overflow or an input/reserve total that
    /// exceeds the verified model and supplied remaining-context limit.
    pub fn admit_model_pass(
        self,
        remaining_context_tokens: u64,
        estimate: ContextTokenEstimate,
        output_reserve_tokens: u64,
    ) -> Result<AdmittedModelPass, ContextAdmissionError> {
        if remaining_context_tokens == 0 || output_reserve_tokens == 0 {
            return Err(ContextAdmissionError::InvalidEstimate);
        }
        let input_tokens = estimate.input_tokens()?;
        let effective_context_limit_tokens = self
            .policy
            .context_window_tokens
            .min(remaining_context_tokens);
        let required_tokens = input_tokens
            .checked_add(self.policy.reasoning_reserve_tokens)
            .and_then(|tokens| tokens.checked_add(output_reserve_tokens))
            .ok_or(ContextAdmissionError::ArithmeticOverflow)?;
        if required_tokens > effective_context_limit_tokens {
            return Err(ContextAdmissionError::ContextLimitExceeded {
                required_tokens,
                available_tokens: effective_context_limit_tokens,
            });
        }
        Ok(AdmittedModelPass {
            input: input_tokens,
            reasoning_reserve: self.policy.reasoning_reserve_tokens,
            output_reserve: output_reserve_tokens,
            effective_context_limit: effective_context_limit_tokens,
        })
    }

    /// Decides whether a result enters context in full or through a compact form.
    ///
    /// `compact_tokens` must describe an already-created bounded representation;
    /// CTX-002 can later supply persisted opaque representations through this seam.
    ///
    /// # Errors
    ///
    /// Returns a recoverable typed rejection if neither representation fits.
    pub const fn admit_tool_result(
        self,
        round: &mut AdmittedToolRound,
        full_tokens: u64,
        compact_tokens: Option<u64>,
    ) -> Result<ToolResultAdmission, ContextAdmissionError> {
        if full_tokens == 0 || matches!(compact_tokens, Some(0)) {
            return Err(ContextAdmissionError::InvalidEstimate);
        }
        let available_tokens = round.available_tool_result_tokens;
        if full_tokens <= available_tokens {
            round.estimate.tool_results += full_tokens;
            round.input_tokens += full_tokens;
            round.available_tool_result_tokens -= full_tokens;
            return Ok(ToolResultAdmission::Full {
                tokens: full_tokens,
            });
        }
        if let Some(tokens) = compact_tokens
            && tokens <= available_tokens
        {
            round.estimate.tool_results += tokens;
            round.input_tokens += tokens;
            round.available_tool_result_tokens -= tokens;
            return Ok(ToolResultAdmission::Compact { tokens });
        }
        Err(ContextAdmissionError::ToolResultTooLarge {
            full_tokens,
            available_tokens,
        })
    }
}

/// Typed, recoverable context-admission failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextAdmissionError {
    /// The trusted model policy is zero or internally impossible.
    InvalidPolicy,
    /// A required trusted token estimate was zero.
    InvalidEstimate,
    /// AGENT-005 disabled tools for this round.
    ToolsDisabled,
    /// AGENT-005's separately held final-answer budget is exhausted.
    FinalResponseReserveExhausted,
    /// The fixed input and explicit reserves exceed the effective context limit.
    ContextLimitExceeded {
        /// Tokens required before admitting any new result.
        required_tokens: u64,
        /// Effective tokens available under model and ledger limits.
        available_tokens: u64,
    },
    /// Neither the full nor optional compact result fits this admitted round.
    ToolResultTooLarge {
        /// Trusted token estimate for the original result.
        full_tokens: u64,
        /// Remaining context capacity after explicit reserves.
        available_tokens: u64,
    },
    /// A component sum exceeded the host accounting type.
    ArithmeticOverflow,
}

impl ContextAdmissionError {
    /// Returns whether the host may recover through compaction or a final-only pass.
    #[must_use]
    pub const fn is_recoverable(self) -> bool {
        matches!(
            self,
            Self::FinalResponseReserveExhausted
                | Self::ContextLimitExceeded { .. }
                | Self::ToolResultTooLarge { .. }
        )
    }
}

impl fmt::Display for ContextAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl Error for ContextAdmissionError {}

#[cfg(test)]
#[path = "context_admission_tests.rs"]
mod tests;
