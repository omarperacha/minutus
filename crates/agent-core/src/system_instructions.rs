//! Trusted, model-neutral system instructions and local knowledge admission.

mod builder;
mod knowledge;
mod types;

pub use builder::build_agent_system_instructions;
pub use types::{
    AGENT_SYSTEM_POLICY_VERSION, AgentSystemInstructionError, AgentSystemInstructionIdentity,
    AgentSystemInstructions, FlowZedKnowledgeSource,
};

#[cfg(test)]
#[path = "system_instructions_tests.rs"]
mod tests;
