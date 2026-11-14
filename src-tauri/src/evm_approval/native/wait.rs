//! Consent cancellation waits for the original native modal's closure receipt.
use crate::wallet_lifecycle::TransferCancellation;
use ipc_types::{EvmApprovalDecision, IpcError, IpcErrorCode};
use tokio::sync::oneshot;

pub(super) async fn decision(
    mut receiver: oneshot::Receiver<Result<EvmApprovalDecision, IpcError>>,
    cancellation: &TransferCancellation,
    abort_owned: impl FnOnce(),
) -> Result<EvmApprovalDecision, IpcError> {
    loop {
        tokio::select! {
            result = &mut receiver => {
                let result = result.map_err(|_| IpcError::internal())?;
                return if cancellation.is_cancelled() { Err(cancelled()) } else { result };
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                if cancellation.is_cancelled() {
                    abort_owned();
                    // No new RPC/effect may start while the cancelled dialog
                    // remains open. Its original sender runs after closure.
                    let _ = receiver.await.map_err(|_| IpcError::internal())?;
                    return Err(cancelled());
                }
            }
        }
    }
}
fn cancelled() -> IpcError {
    IpcError {
        code: IpcErrorCode::Cancelled,
        message: "Native consent was cancelled".into(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn read_consent_cancellation_aborts_once_and_waits_for_original_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let requested = Arc::new(AtomicBool::new(true));
        let cancellation = TransferCancellation::new(requested);
        let (sender, receiver) = oneshot::channel();
        let (aborted, abort_received) = oneshot::channel();
        let returned = Arc::new(AtomicBool::new(false));
        let completed = Arc::clone(&returned);
        let waiting = async {
            let result = decision(receiver, &cancellation, || {
                let _ = aborted.send(());
            })
            .await;
            completed.store(true, Ordering::Release);
            result
        };
        let native = async {
            abort_received.await?;
            assert!(!returned.load(Ordering::Acquire));
            sender
                .send(Ok(EvmApprovalDecision::Dismissed))
                .map_err(|_| "receiver closed before native cleanup")?;
            Ok::<(), Box<dyn std::error::Error>>(())
        };
        let (result, native) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(waiting, native)
        })
        .await?;
        native?;
        assert!(matches!(
            result,
            Err(IpcError {
                code: IpcErrorCode::Cancelled,
                ..
            })
        ));
        Ok(())
    }
    #[tokio::test]
    async fn a_late_approval_cannot_override_request_cancellation()
    -> Result<(), Box<dyn std::error::Error>> {
        let requested = Arc::new(AtomicBool::new(false));
        let cancellation = TransferCancellation::new(Arc::clone(&requested));
        let (sender, receiver) = oneshot::channel();
        requested.store(true, Ordering::Release);
        sender
            .send(Ok(EvmApprovalDecision::Approved))
            .map_err(|_| "send failed")?;
        assert!(matches!(
            decision(receiver, &cancellation, || {}).await,
            Err(IpcError {
                code: IpcErrorCode::Cancelled,
                ..
            })
        ));
        Ok(())
    }
}
