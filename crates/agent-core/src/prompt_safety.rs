//! Trusted prompt-injection inspection and authority-free indicator contracts.

mod detector;
mod types;

pub use detector::PromptInjectionDetector;
pub use types::{
    BoundedResourceReference, InjectionSignal, PromptInjectionIndicator, PromptSafetyError,
    UntrustedContentSource, UntrustedSourceKind,
};

#[cfg(test)]
#[path = "prompt_safety_tests.rs"]
mod tests;
