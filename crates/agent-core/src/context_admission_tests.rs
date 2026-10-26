use super::*;
use crate::ExecutionBudgets;

fn allowance(context_tokens: u64, final_response_tokens: u64) -> RoundAllowance {
    RoundAllowance {
        remaining: ExecutionBudgets {
            rounds: 2,
            tool_calls: 4,
            elapsed_ms: 10_000,
            tool_ms: 5_000,
            context_tokens,
            final_response_tokens,
            tool_output_bytes: 16_000,
        },
        tools_enabled: true,
    }
}

fn every_category() -> ContextTokenEstimate {
    ContextTokenEstimate {
        system_instructions: 10,
        conversation_history: 20,
        tool_definitions: 30,
        tool_calls: 40,
        tool_results: 50,
        images: 60,
    }
}

#[test]
fn estimate_accounts_for_every_model_visible_category() -> Result<(), ContextAdmissionError> {
    assert_eq!(every_category().input_tokens()?, 210);
    Ok(())
}

#[test]
fn exact_boundary_reserves_reasoning_and_final_response_before_tools()
-> Result<(), ContextAdmissionError> {
    let controller = ContextAdmissionController::new(ContextAdmissionPolicy::new(1_000, 200)?);
    let mut round = controller.admit_tool_round(allowance(1_000, 300), every_category())?;
    assert_eq!(round.input_tokens(), 210);
    assert_eq!(round.reasoning_reserve_tokens(), 200);
    assert_eq!(round.final_response_reserve_tokens(), 300);
    assert_eq!(round.available_tool_result_tokens(), 290);
    assert_eq!(
        controller.admit_tool_result(&mut round, 290, None)?,
        ToolResultAdmission::Full { tokens: 290 }
    );
    assert_eq!(round.available_tool_result_tokens(), 0);
    assert_eq!(round.estimate().tool_results, 340);
    Ok(())
}

#[test]
fn one_token_over_boundary_uses_compact_form_or_returns_recoverable_rejection()
-> Result<(), ContextAdmissionError> {
    let controller = ContextAdmissionController::new(ContextAdmissionPolicy::new(1_000, 200)?);
    let mut round = controller.admit_tool_round(allowance(1_000, 300), every_category())?;
    let Err(error) = controller.admit_tool_result(&mut round, 291, None) else {
        return Err(ContextAdmissionError::InvalidEstimate);
    };
    assert_eq!(
        error,
        ContextAdmissionError::ToolResultTooLarge {
            full_tokens: 291,
            available_tokens: 290,
        }
    );
    assert!(error.is_recoverable());
    assert_eq!(
        controller.admit_tool_result(&mut round, 291, Some(290))?,
        ToolResultAdmission::Compact { tokens: 290 }
    );
    assert_eq!(round.available_tool_result_tokens(), 0);
    Ok(())
}

#[test]
fn remaining_agent_budget_caps_the_model_window_without_a_second_counter()
-> Result<(), ContextAdmissionError> {
    let controller = ContextAdmissionController::new(ContextAdmissionPolicy::new(8_000, 200)?);
    let round = controller.admit_tool_round(allowance(800, 300), every_category())?;
    assert_eq!(round.effective_context_limit_tokens(), 800);
    assert_eq!(round.available_tool_result_tokens(), 90);
    Ok(())
}

#[test]
fn fixed_prompt_exact_boundary_is_admitted_and_one_token_over_is_rejected()
-> Result<(), ContextAdmissionError> {
    let controller = ContextAdmissionController::new(ContextAdmissionPolicy::new(710, 200)?);
    let exact = controller.admit_tool_round(allowance(710, 300), every_category())?;
    assert_eq!(exact.available_tool_result_tokens(), 0);

    let controller = ContextAdmissionController::new(ContextAdmissionPolicy::new(709, 200)?);
    assert_eq!(
        controller.admit_tool_round(allowance(709, 300), every_category()),
        Err(ContextAdmissionError::ContextLimitExceeded {
            required_tokens: 710,
            available_tokens: 709,
        })
    );
    Ok(())
}

#[test]
fn auxiliary_model_pass_has_its_own_exact_reserves() -> Result<(), ContextAdmissionError> {
    let estimate = ContextTokenEstimate {
        conversation_history: 512,
        ..ContextTokenEstimate::default()
    };
    let exact = ContextAdmissionController::new(ContextAdmissionPolicy::new(1_024, 256)?)
        .admit_model_pass(1_024, estimate, 256)?;
    assert_eq!(exact.input_tokens(), 512);
    assert_eq!(exact.reasoning_reserve_tokens(), 256);
    assert_eq!(exact.output_reserve_tokens(), 256);
    assert_eq!(exact.effective_context_limit_tokens(), 1_024);

    assert_eq!(
        ContextAdmissionController::new(ContextAdmissionPolicy::new(1_024, 256)?)
            .admit_model_pass(1_023, estimate, 256),
        Err(ContextAdmissionError::ContextLimitExceeded {
            required_tokens: 1_024,
            available_tokens: 1_023,
        })
    );
    Ok(())
}

#[test]
fn disabled_tools_empty_final_reserve_and_overflow_fail_closed() -> Result<(), ContextAdmissionError>
{
    let controller = ContextAdmissionController::new(ContextAdmissionPolicy::new(1_000, 100)?);
    let mut disabled = allowance(1_000, 100);
    disabled.tools_enabled = false;
    assert_eq!(
        controller.admit_tool_round(disabled, ContextTokenEstimate::default()),
        Err(ContextAdmissionError::ToolsDisabled)
    );
    assert_eq!(
        controller.admit_tool_round(allowance(1_000, 0), ContextTokenEstimate::default()),
        Err(ContextAdmissionError::FinalResponseReserveExhausted)
    );
    let overflow = ContextTokenEstimate {
        system_instructions: u64::MAX,
        conversation_history: 1,
        ..ContextTokenEstimate::default()
    };
    assert_eq!(
        controller.admit_tool_round(allowance(1_000, 100), overflow),
        Err(ContextAdmissionError::ArithmeticOverflow)
    );
    let mut round =
        controller.admit_tool_round(allowance(1_000, 100), ContextTokenEstimate::default())?;
    assert_eq!(
        controller.admit_tool_result(&mut round, 0, None),
        Err(ContextAdmissionError::InvalidEstimate)
    );
    assert_eq!(
        controller.admit_tool_result(&mut round, 901, Some(0)),
        Err(ContextAdmissionError::InvalidEstimate)
    );
    Ok(())
}

#[test]
fn multiple_results_consume_one_cumulative_round_allowance() -> Result<(), ContextAdmissionError> {
    let controller = ContextAdmissionController::new(ContextAdmissionPolicy::new(1_000, 200)?);
    let mut round = controller.admit_tool_round(allowance(1_000, 300), every_category())?;
    assert_eq!(
        controller.admit_tool_result(&mut round, 200, None)?,
        ToolResultAdmission::Full { tokens: 200 }
    );
    assert_eq!(round.available_tool_result_tokens(), 90);
    assert_eq!(
        controller.admit_tool_result(&mut round, 91, None),
        Err(ContextAdmissionError::ToolResultTooLarge {
            full_tokens: 91,
            available_tokens: 90,
        })
    );
    assert_eq!(round.available_tool_result_tokens(), 90);
    Ok(())
}

#[test]
fn invalid_policy_is_rejected() {
    assert_eq!(
        ContextAdmissionPolicy::new(0, 1),
        Err(ContextAdmissionError::InvalidPolicy)
    );
    assert_eq!(
        ContextAdmissionPolicy::new(100, 100),
        Err(ContextAdmissionError::InvalidPolicy)
    );
}
