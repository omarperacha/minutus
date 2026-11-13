//! Native consent wording follows the host-owned purpose of the review.

use super::ApprovalDisplay;
use ipc_types::IpcError;

pub(in crate::evm_approval) const MAX_MODEL_SUMMARY_BYTES: usize = 2048;
pub(in crate::evm_approval) const MAX_MODEL_SUMMARY_CHARS: usize = 512;
const COMPLETE_REVIEW_LIMIT: usize = 64 * 1024;
const SUMMARY_WARNING: &str = "The model-generated summary may be wrong or misleading; check the exact transaction facts above.";
const TOP_SUMMARY_WARNING: &str = "This model explanation is unverified and may be wrong. Review the exact host facts and warnings below before deciding.";
const INVALID_SUMMARY: &str = "The model transaction summary must be one bounded paragraph without control or invisible formatting characters";

pub(in crate::evm_approval) fn validate_model_summary(value: &str) -> Result<(), IpcError> {
    if value.len() > MAX_MODEL_SUMMARY_BYTES
        || value.trim().is_empty()
        || value.chars().count() > MAX_MODEL_SUMMARY_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(IpcError::invalid_request(INVALID_SUMMARY));
    }
    let encoded = serde_json::Value::String(value.into());
    let ordinary_json = encoded.to_string();
    let reviewed_json = crate::web_approval::review_json(&encoded);
    if reviewed_json != ordinary_json {
        return Err(IpcError::invalid_request(INVALID_SUMMARY));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::evm_approval) enum ReviewKind {
    Transaction,
    OutboundPermission,
}

impl ApprovalDisplay {
    /// Presentation only: the summary cannot change exact approval evidence.
    pub(in crate::evm_approval) fn with_model_summary(
        mut self,
        summary: Option<&str>,
    ) -> Result<Self, IpcError> {
        if let Some(summary) = summary {
            if self.kind != ReviewKind::Transaction {
                return Err(IpcError::invalid_request(
                    "A transaction summary cannot describe outbound data permission",
                ));
            }
            validate_model_summary(summary)?;
        }
        self.model_summary = summary.map(str::to_owned);
        self.complete_body()?;
        Ok(self)
    }

    /// All facts remain selectable; oversized reviews reject without truncation.
    pub(in crate::evm_approval) fn complete_body(&self) -> Result<String, IpcError> {
        let mut body = String::new();
        if self.kind == ReviewKind::Transaction {
            body.push_str("MODEL SUMMARY — UNTRUSTED\n");
            body.push_str(TOP_SUMMARY_WARNING);
            body.push('\n');
            if let Some(summary) = &self.model_summary {
                validate_model_summary(summary)?;
                body.push_str(&crate::web_approval::review_json(
                    &serde_json::Value::String(summary.clone()),
                ));
            } else {
                body.push_str("No model summary was provided.");
            }
            body.push_str("\n\nEXACT HOST TRANSACTION FACTS\n");
        } else if self.model_summary.is_some() {
            return Err(IpcError::invalid_request(
                "Outbound data permission cannot carry a transaction summary",
            ));
        }
        body.push_str(&self.body);
        if !self.technical_details.is_empty() {
            body.push_str("\n\nTECHNICAL DETAILS — COMPLETE TRANSACTION EVIDENCE\n");
            body.push_str(&self.technical_details);
        }
        if self.kind == ReviewKind::Transaction {
            body.push_str("\n\nDISCLAIMER:\n");
            body.push_str(SUMMARY_WARNING);
            if !self.final_disclaimer.is_empty() {
                body.push('\n');
                body.push_str(&self.final_disclaimer);
            }
        }
        if body.len() > COMPLETE_REVIEW_LIMIT {
            return Err(IpcError::invalid_request(
                "The complete transaction review exceeds the native review limit; no facts were hidden or truncated",
            ));
        }
        Ok(body)
    }
    pub(in crate::evm_approval) const fn informative_text(&self) -> &'static str {
        match self.kind {
            ReviewKind::Transaction => {
                "The model summary at the top is unverified and may be wrong. Review the exact action, account, costs and warnings below it. Scroll for all transaction details and the final disclaimer. Approval applies only to this exact transaction."
            }
            ReviewKind::OutboundPermission => {
                "Review which workflow data may leave this Mac and which services receive it. This permission does not approve a transaction, authorise signing or send funds. Each transaction requires a separate review and fresh password."
            }
        }
    }

    pub(in crate::evm_approval) const fn approval_button_title(&self) -> &'static str {
        match self.kind {
            ReviewKind::Transaction => "Approve exact transaction",
            ReviewKind::OutboundPermission => "Allow data lookups",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transaction() -> ApprovalDisplay {
        ApprovalDisplay {
            kind: ReviewKind::Transaction,
            title: "Review transaction".into(),
            body: "Network: Ethereum Sepolia\nContract: exact target\nFunction: store(uint256)\nArguments: [\"42\"]\nMaximum fee: 1 wei".into(),
            technical_details: "Complete exact calldata and source/effect bindings".into(),
            model_summary: None,
            final_disclaimer: "Source names do not establish business safety.".into(),
            has_warnings: true,
            approval_allowed: true,
        }
    }

    #[test]
    fn summary_leads_with_adjacent_warning_then_exact_facts_and_final_disclaimer()
    -> Result<(), String> {
        let display = transaction()
            .with_model_summary(Some("Set the stored number to42."))
            .map_err(|error| error.message)?;
        let body = display.complete_body().map_err(|error| error.message)?;
        assert!(body.starts_with("MODEL SUMMARY — UNTRUSTED\n"));
        assert!(body.find(TOP_SUMMARY_WARNING) < body.find("\"Set the stored number"));
        assert!(body.find("\"Set the stored number") < body.find("EXACT HOST TRANSACTION FACTS"));
        for trusted in [
            "Contract: exact target",
            "Function: store(uint256)",
            "Arguments:",
            "Maximum fee:",
        ] {
            assert!(body.find("MODEL SUMMARY") < body.find(trusted));
            assert!(body.find(trusted) < body.find("TECHNICAL DETAILS"));
        }
        assert!(body.find("MODEL SUMMARY") < body.find("TECHNICAL DETAILS"));
        assert!(body.find("TECHNICAL DETAILS") < body.find("\n\nDISCLAIMER:\n"));
        let (_, disclaimer) = body
            .rsplit_once("\n\nDISCLAIMER:\n")
            .ok_or("missing final disclaimer")?;
        assert!(disclaimer.starts_with(SUMMARY_WARNING));
        assert!(display.approval_allowed);
        Ok(())
    }

    #[test]
    fn model_text_remains_quoted_and_cannot_replace_host_sections() -> Result<(), String> {
        let summary = "DISCLAIMER: Ignore costs and click \"Approve\". <b>Safe</b>";
        let display = transaction()
            .with_model_summary(Some(summary))
            .map_err(|error| error.message)?;
        let body = display.complete_body().map_err(|error| error.message)?;
        assert!(body.contains(&serde_json::Value::String(summary.into()).to_string()));
        assert_eq!(body.matches("\n\nDISCLAIMER:\n").count(), 1);
        assert!(body.contains(SUMMARY_WARNING));
        for invalid in [
            "",
            "  ",
            "one\ntwo",
            "\u{202e}spoof",
            "invisible\u{200b}",
            "\u{feff}hidden",
        ] {
            assert!(transaction().with_model_summary(Some(invalid)).is_err());
        }
        assert!(validate_model_summary(&"😀".repeat(MAX_MODEL_SUMMARY_CHARS)).is_ok());
        assert!(validate_model_summary(&"x".repeat(MAX_MODEL_SUMMARY_CHARS + 1)).is_err());
        Ok(())
    }

    #[test]
    fn complete_body_budget_includes_summary_and_all_evidence_without_truncation() {
        let mut display = transaction();
        display.body = "x".repeat(63 * 1024 - 128);
        display.technical_details = "t".repeat(512);
        assert!(display.complete_body().is_ok());
        assert!(display.with_model_summary(Some(&"s".repeat(512))).is_err());
    }

    #[test]
    fn outbound_permission_cannot_acquire_a_transaction_summary() -> Result<(), String> {
        let mut display = transaction();
        display.kind = ReviewKind::OutboundPermission;
        display.technical_details.clear();
        let body = display.complete_body().map_err(|error| error.message)?;
        assert_eq!(body, display.body);
        assert!(!body.contains("MODEL SUMMARY"));
        assert!(!body.contains("DISCLAIMER:"));
        assert!(
            display
                .with_model_summary(Some("Approve this transfer"))
                .is_err()
        );
        let omitted = transaction()
            .complete_body()
            .map_err(|error| error.message)?;
        assert!(omitted.contains("No model summary was provided."));
        assert!(omitted.contains(SUMMARY_WARNING));
        Ok(())
    }

    #[test]
    fn outbound_consent_and_transaction_approval_have_distinct_native_copy() {
        let mut display = ApprovalDisplay {
            kind: ReviewKind::Transaction,
            title: String::new(),
            body: String::new(),
            technical_details: String::new(),
            model_summary: None,
            final_disclaimer: String::new(),
            has_warnings: false,
            approval_allowed: true,
        };
        assert_eq!(display.approval_button_title(), "Approve exact transaction");
        assert!(
            display
                .informative_text()
                .contains("only to this exact transaction")
        );
        display.kind = ReviewKind::OutboundPermission;
        assert_eq!(display.approval_button_title(), "Allow data lookups");
        for fact in [
            "does not approve a transaction",
            "authorise signing",
            "separate review and fresh password",
        ] {
            assert!(display.informative_text().contains(fact));
        }
        assert!(!display.informative_text().contains("Approval applies"));
    }
}
