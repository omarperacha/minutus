//! Initial native wallet readiness precedes account-bound scope construction.
//! Unlocking grants neither outbound consent nor fresh signing authentication.
use crate::wallet_lifecycle::TransferCancellation;
use chain_evm::PublicEvmAddress;
use ipc_types::IpcError;
use std::future::Future;

pub(super) async fn initial_account(
    cancel: &TransferCancellation,
    ensure_ready: impl Future<Output = Result<String, IpcError>>,
) -> Result<String, IpcError> {
    cancel.check()?;
    let ready = ensure_ready.await;
    // The transfer owner aborts a pending native prompt on cancellation. Check
    // again before a completed unlock can supply an account for outbound scope.
    cancel.check()?;
    let account = ready?;
    PublicEvmAddress::new(&account)
        .map(|address| address.as_str().to_owned())
        .map_err(|_| IpcError::invalid_request("The selected public wallet address is invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc_types::IpcErrorCode;
    use std::{
        cell::Cell,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    const ACCOUNT: &str = "0x52908400098527886E0F7030069857D2E4169EE7";

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_request_never_polls_native_readiness() {
        let cancel = TransferCancellation::new(Arc::new(AtomicBool::new(true)));
        let prompted = Cell::new(false);
        let result = initial_account(&cancel, async {
            prompted.set(true);
            Ok(ACCOUNT.into())
        })
        .await;
        assert!(!prompted.get());
        assert_eq!(
            result.err().map(|error| error.code),
            Some(IpcErrorCode::Cancelled)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_during_unlock_cannot_supply_a_scope_account() {
        let requested = Arc::new(AtomicBool::new(false));
        let cancel = TransferCancellation::new(Arc::clone(&requested));
        let result = initial_account(&cancel, async {
            requested.store(true, Ordering::Release);
            Ok(ACCOUNT.into())
        })
        .await;
        assert_eq!(
            result.err().map(|error| error.code),
            Some(IpcErrorCode::Cancelled)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_unlock_cancellation_and_unavailable_wallet_errors_are_preserved() {
        let cancel = TransferCancellation::new(Arc::new(AtomicBool::new(false)));
        for error in [
            IpcError {
                code: IpcErrorCode::Cancelled,
                message: "Wallet operation cancelled".into(),
                retryable: false,
            },
            crate::wallet_lifecycle::wallet_error(
                "Create, restore or import an account first",
                false,
            ),
        ] {
            let expected = (error.code.clone(), error.message.clone(), error.retryable);
            let result = initial_account(&cancel, async { Err(error) }).await;
            assert_eq!(
                result
                    .err()
                    .map(|error| (error.code, error.message, error.retryable)),
                Some(expected)
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_readiness_finishes_before_the_canonical_account_is_released() {
        let cancel = TransferCancellation::new(Arc::new(AtomicBool::new(false)));
        let unlocked = Cell::new(false);
        let result = initial_account(&cancel, async {
            unlocked.set(true);
            Ok(ACCOUNT.into())
        })
        .await;
        assert!(unlocked.get());
        assert_eq!(
            result.ok().as_deref(),
            Some("0x52908400098527886e0f7030069857d2e4169ee7")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn invalid_readiness_address_cannot_supply_a_scope_account() {
        let cancel = TransferCancellation::new(Arc::new(AtomicBool::new(false)));
        let result = initial_account(&cancel, async { Ok("not-an-address".into()) }).await;
        assert_eq!(
            result.err().map(|error| error.code),
            Some(IpcErrorCode::InvalidRequest)
        );
    }
}
