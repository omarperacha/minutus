//! Speech setup and, subsequently, application-owned dictation coordination.

mod admission;
mod installation;
mod session;

pub use admission::ChatActivityLease;
pub(crate) use admission::ForegroundActivityGate;
pub(super) use installation::SpeechModels;
pub use session::DictationCancelReason;
pub(crate) use session::DictationSessions;

#[cfg(test)]
mod tests;
