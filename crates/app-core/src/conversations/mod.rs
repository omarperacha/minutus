//! Conversation history, messages, titles, and managed-asset lifecycle.

mod assets;
mod history;

#[cfg(test)]
pub(super) use history::derive_conversation_title;
