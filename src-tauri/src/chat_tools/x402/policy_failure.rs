//! Bounded policy diagnostics for the pre-payment challenge boundary.

use ipc_types::IpcError;
use x402_client::X402PolicyError;

pub(super) fn after_unpaid_challenge(error: X402PolicyError) -> IpcError {
    super::terminal(&format!(
        "The host rejected the x402 challenge ({}): {error}. \
         The unpaid request was sent, but this attempt stopped before payment approval, \
         signing, or a paid retry. The endpoint may already have created a pending \
         quote or order; its remote state has not been verified.",
        error.code()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_identifies_the_rule_without_claiming_remote_rollback() {
        for error in [
            X402PolicyError::UnsupportedExtension,
            X402PolicyError::InvalidExtensionData,
            X402PolicyError::UnsupportedNetwork,
            X402PolicyError::UnsupportedAsset,
        ] {
            let failure = after_unpaid_challenge(error);
            assert!(!failure.retryable);
            assert!(failure.message.contains(error.code()));
            assert!(failure.message.contains("before payment approval"));
            assert!(failure.message.contains("unpaid request was sent"));
            assert!(
                failure
                    .message
                    .contains("remote state has not been verified")
            );
            assert!(!failure.message.contains("no order"));
            assert!(failure.message.len() < 512);
        }
    }
}
