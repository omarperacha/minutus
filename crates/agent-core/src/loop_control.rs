mod controller;
mod types;

pub use controller::AgentLoopController;
pub use types::{
    AgentLoopSnapshot, FinalAnswerReason, GenerationUsage, LoopControlError, LoopDirective,
    LoopLimits, RoundAllowance, ToolCallIntent, ToolCallPlan, ToolExecutionUsage,
};

#[cfg(test)]
#[path = "loop_control_tests.rs"]
mod tests;
