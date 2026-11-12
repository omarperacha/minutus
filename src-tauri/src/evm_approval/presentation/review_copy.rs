//! Native consent wording follows the host-owned purpose of the review.

use super::ApprovalDisplay;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::evm_approval) enum ReviewKind {
    Transaction,
    OutboundPermission,
}

impl ApprovalDisplay {
    pub(in crate::evm_approval) const fn informative_text(&self) -> &'static str {
        match self.kind {
            ReviewKind::Transaction => {
                "Review the network, account, recipient, amount, maximum fees and warnings below. Scroll for the complete transaction and technical details. Approval applies only to this exact transaction."
            }
            ReviewKind::OutboundPermission => {
                "Review which workflow data may leave this Mac and which services receive it. This permission does not approve a transaction, authorise signing or send funds. Each transaction requires a separate review and fresh password."
            }
        }
    }

    pub(in crate::evm_approval) const fn approval_button_title(&self) -> &'static str {
        match self.kind {
            ReviewKind::Transaction => "Approve exact transaction",
            ReviewKind::OutboundPermission => "Allow outbound data",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_consent_and_transaction_approval_have_distinct_native_copy() {
        let mut display = ApprovalDisplay {
            kind: ReviewKind::Transaction,
            title: String::new(),
            body: String::new(),
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
        assert_eq!(display.approval_button_title(), "Allow outbound data");
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
