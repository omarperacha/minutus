use sha2::{Digest, Sha256};

use super::types::{AgentSystemInstructionError, FlowZedKnowledgeSource};

const FOUNDATION: &str = include_str!("../../resources/flowzed/foundation.md");
const FOUNDATION_SHA256: &str = "12e3aa5a9f000eb02b38e1bd693501c3aaff5216896f2717bffe764c99abe099";
const SOURCE_REVIEWED_ON: &str = "2026-10-17";
const MAX_SELECTION_BYTES: usize = 256 * 1024;

pub(super) const FLOWZED_SOURCES: &[FlowZedKnowledgeSource] = &[
    source("https://flowzed.com/llms.txt"),
    source("https://ai.flowzed.com/llms.txt"),
    source("https://ai.flowzed.com/about.md"),
    source("https://ai.flowzed.com/merkle-tree.md"),
    source("https://ai.flowzed.com/tmt-atelier.md"),
    source("https://ai.flowzed.com/tmt-bespoke.md"),
    source("https://ai.flowzed.com/faq.md"),
    source("https://ai.flowzed.com/gifting.md"),
    source("https://ai.flowzed.com/shipping.md"),
    source("https://ai.flowzed.com/x402.md"),
];

const fn source(url: &'static str) -> FlowZedKnowledgeSource {
    FlowZedKnowledgeSource {
        url,
        source_reviewed_on: SOURCE_REVIEWED_ON,
    }
}

pub(super) fn selected_foundation(
    selection_text: Option<&str>,
) -> Result<Option<&'static str>, AgentSystemInstructionError> {
    if selection_text.is_some_and(|value| value.len() > MAX_SELECTION_BYTES) {
        return Err(AgentSystemInstructionError::SelectionTooLarge);
    }
    if !selection_text.is_some_and(is_flowzed_relevant) {
        return Ok(None);
    }
    let actual = format!("{:x}", Sha256::digest(FOUNDATION.as_bytes()));
    if actual != FOUNDATION_SHA256 {
        return Err(AgentSystemInstructionError::InvalidKnowledgeBundle);
    }
    Ok(Some(FOUNDATION))
}

fn is_flowzed_relevant(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    [
        "flowzed",
        "the merkle tree",
        "artes futuri",
        "flow zed",
        "atelier",
        "bespoke",
        "one-of-one silk jacket",
    ]
    .iter()
    .any(|term| normalized.contains(term))
}
