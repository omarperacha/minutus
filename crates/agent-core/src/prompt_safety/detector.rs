use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose};
use serde_json::Value;

use super::types::{
    InjectionSignal, PromptInjectionIndicator, PromptSafetyError, UntrustedContentSource,
};

const MAX_CONTENT_BYTES: usize = 1024 * 1024;
const MAX_JSON_DEPTH: usize = 32;
const MAX_JSON_STRINGS: usize = 2_048;
const MAX_ENCODED_CANDIDATE_BYTES: usize = 8 * 1024;

/// Stateless trusted-host detector for advisory prompt-injection indicators.
#[derive(Clone, Copy, Debug, Default)]
pub struct PromptInjectionDetector;

impl PromptInjectionDetector {
    /// Inspects bounded untrusted text and returns an authority-free indicator.
    ///
    /// An empty result means no configured signal was found, not that the
    /// content is trusted. Schema, capability, permission and sandbox controls
    /// remain mandatory in either case.
    ///
    /// # Errors
    ///
    /// Returns [`PromptSafetyError::ContentTooLarge`] when the input exceeds
    /// the trusted host's inspection bound.
    pub fn inspect_text(
        self,
        source: UntrustedContentSource,
        content: &str,
    ) -> Result<Option<PromptInjectionIndicator>, PromptSafetyError> {
        if content.len() > MAX_CONTENT_BYTES {
            return Err(PromptSafetyError::ContentTooLarge);
        }
        Ok(indicator(source, scan_text(content)))
    }

    /// Recursively inspects bounded JSON keys and string values.
    ///
    /// # Errors
    ///
    /// Returns an error when serialized size, traversal depth or string count
    /// exceeds the trusted host's inspection bounds.
    pub fn inspect_json(
        self,
        source: UntrustedContentSource,
        content: &Value,
    ) -> Result<Option<PromptInjectionIndicator>, PromptSafetyError> {
        if serde_json::to_vec(content)
            .map_err(|_| PromptSafetyError::ContentTooLarge)?
            .len()
            > MAX_CONTENT_BYTES
        {
            return Err(PromptSafetyError::ContentTooLarge);
        }
        let mut state = JsonScanState::default();
        scan_json(content, 0, &mut state)?;
        Ok(indicator(source, state.signals))
    }
}

fn indicator(
    source: UntrustedContentSource,
    signals: BTreeSet<InjectionSignal>,
) -> Option<PromptInjectionIndicator> {
    (!signals.is_empty())
        .then(|| PromptInjectionIndicator::new(source, signals.into_iter().collect()))
}

#[derive(Default)]
struct JsonScanState {
    strings: usize,
    signals: BTreeSet<InjectionSignal>,
}

fn scan_json(
    value: &Value,
    depth: usize,
    state: &mut JsonScanState,
) -> Result<(), PromptSafetyError> {
    if depth > MAX_JSON_DEPTH {
        return Err(PromptSafetyError::ContentTooDeep);
    }
    match value {
        Value::String(value) => scan_json_string(value, state)?,
        Value::Array(values) => {
            for value in values {
                scan_json(value, depth + 1, state)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                scan_json_string(key, state)?;
                scan_json(value, depth + 1, state)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn scan_json_string(value: &str, state: &mut JsonScanState) -> Result<(), PromptSafetyError> {
    state.strings = state.strings.saturating_add(1);
    if state.strings > MAX_JSON_STRINGS {
        return Err(PromptSafetyError::ContentTooLarge);
    }
    state.signals.extend(scan_text(value));
    Ok(())
}

fn scan_text(content: &str) -> BTreeSet<InjectionSignal> {
    let mut signals = scan_plain_text(content);
    for token in encoded_candidates(content) {
        if let Some(decoded) = decode_candidate(token) {
            let decoded_signals = scan_plain_text(&decoded);
            if !decoded_signals.is_empty() {
                signals.insert(InjectionSignal::EncodedInstruction);
                signals.extend(decoded_signals);
            }
        }
    }
    signals
}

fn scan_plain_text(content: &str) -> BTreeSet<InjectionSignal> {
    let normalized = content
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if is_false_positive_context(&normalized) {
        return BTreeSet::new();
    }

    let mut signals = BTreeSet::new();
    if contains_any(
        &normalized,
        &[
            "ignore previous instructions",
            "ignore all previous instructions",
            "disregard previous instructions",
            "disregard all prior instructions",
            "override system instructions",
            "replace the system prompt",
            "you are now the system",
            "follow my instructions instead",
        ],
    ) {
        signals.insert(InjectionSignal::InstructionOverride);
    }
    if contains_any(
        &normalized,
        &[
            "approve this request",
            "approve the request",
            "grant me permission",
            "permission is already granted",
            "you have permission to",
            "bypass the approval",
            "bypass permission",
            "grant this capability",
        ],
    ) {
        signals.insert(InjectionSignal::AuthorityEscalation);
    }
    let exfiltration_verb = contains_any(
        &normalized,
        &[
            "send ", "upload ", "reveal ", "expose ", "return ", "print ",
        ],
    );
    let sensitive_target = contains_any(
        &normalized,
        &[
            "api key",
            "password",
            "credential",
            "secret",
            ".env",
            "private key",
        ],
    );
    if exfiltration_verb && sensitive_target {
        signals.insert(InjectionSignal::SensitiveDataExfiltration);
    }
    signals
}

fn is_false_positive_context(normalized: &str) -> bool {
    let suspicious_phrase_is_discussed = contains_any(
        normalized,
        &[
            "prompt injection example",
            "example of prompt injection",
            "detect prompt injection",
            "prompt-injection detector",
            "the phrase \"ignore",
            "the phrase 'ignore",
            "documentation says",
        ],
    );
    suspicious_phrase_is_discussed
        && !contains_any(
            normalized,
            &[
                "do this now",
                "actually ignore",
                "must obey",
                "immediately approve",
            ],
        )
}

fn encoded_candidates(content: &str) -> impl Iterator<Item = &str> {
    content
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=' | '-' | '_'))
        })
        .filter(|candidate| candidate.len() >= 20 && candidate.len() <= MAX_ENCODED_CANDIDATE_BYTES)
}

fn decode_candidate(candidate: &str) -> Option<String> {
    let decoded = general_purpose::STANDARD
        .decode(candidate)
        .or_else(|_| general_purpose::URL_SAFE.decode(candidate))
        .ok()?;
    if decoded.is_empty()
        || decoded.len() > MAX_ENCODED_CANDIDATE_BYTES
        || decoded
            .iter()
            .filter(|byte| byte.is_ascii_graphic() || byte.is_ascii_whitespace())
            .count()
            * 10
            < decoded.len() * 9
    {
        return None;
    }
    String::from_utf8(decoded).ok()
}

fn contains_any(content: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| content.contains(pattern))
}
