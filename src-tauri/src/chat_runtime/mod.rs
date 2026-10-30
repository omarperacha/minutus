//! Chat commands, generation orchestration, and timeline events.

mod commands;
mod events;
mod generation;
mod plan;
mod recovery;
mod rich_input;

#[cfg(debug_assertions)]
mod efficiency_eval;

#[cfg(debug_assertions)]
pub(crate) use efficiency_eval::{
    run as run_efficiency_eval, tool_denial as evaluation_tool_denial,
};

pub(crate) use commands::{start_chat_generation, stop_chat_generation};
pub(crate) use events::{emit_chat_event, next_sequence, tool_activity};

#[cfg(not(debug_assertions))]
pub(crate) use plan::qualify_contract_release_surface;
