//! Fresh app-password authentication followed by the existing exact approval gate.
use super::{
    WalletControl, WalletSigned, WalletSubmission, blocking, cancelled_error, native, onboarding,
    password_error, wallet_error,
};
use app_core::AppCore;
use ipc_types::IpcError;
use std::{sync::atomic::Ordering, time::Instant};
use transaction_policy::{ApprovalCommitment, ApprovalInvalidationReason};
use wallet_core::{
    AppVaultDocument, AppVaultFreshSigning, EvmSigningObservation, PasswordVaultError as Error,
    PasswordVaultJob, VaultKeyId,
};

// Secret authentication owners drop before admission, including a discarded worker result.
struct PreparedSign {
    document: AppVaultDocument,
    id: VaultKeyId,
    authentication: AppVaultFreshSigning,
    epoch: u64,
    job: PasswordVaultJob,
}
impl WalletControl {
    pub(crate) async fn unlock_native(&self, app: &tauri::AppHandle) -> Result<(), IpcError> {
        let job = self.begin()?;
        let (document, id) = self.snapshot()?;
        let document = document
            .ok_or_else(|| wallet_error("Create, restore or import an account first", false))?;
        let id = id.ok_or_else(IpcError::internal)?;
        let password = onboarding::password(app, native::PasswordPrompt::Enter, &job).await?;
        let control = self.clone();
        blocking(move || control.unlock_blocking(document, id, &password, &job)).await
    }
    pub(crate) async fn ensure_x402_account_ready(
        &self,
        core: &AppCore,
        network: chain_evm::EvmNetwork,
    ) -> Result<x402_client::ActiveX402WalletAccount, IpcError> {
        let address = self.ensure_account_ready(core).await?;
        x402_client::ActiveX402WalletAccount::new(&address, network)
            .map_err(|_| wallet_error("The selected account is unavailable", false))
    }
    pub(crate) async fn ensure_account_ready(&self, core: &AppCore) -> Result<String, IpcError> {
        if self.public_state()?.availability != ipc_types::WalletAvailability::Ready {
            self.unlock_native(self.app()?).await?;
            super::persist_public(core, self)?;
        }
        self.selected_address()
    }
    async fn prepare_sign(&self) -> Result<PreparedSign, IpcError> {
        let job = self.begin()?;
        let (document, id) = self.snapshot()?;
        if self.public_state()?.availability != ipc_types::WalletAvailability::Ready {
            return Err(wallet_error("Unlock the app vault before approval", false));
        }
        let document = document.ok_or_else(IpcError::internal)?;
        let id = id.ok_or_else(IpcError::internal)?;
        let password =
            onboarding::password(self.app()?, native::PasswordPrompt::Enter, &job).await?;
        let control = self.clone();
        let epoch = self.epoch.load(Ordering::Acquire);
        blocking(move || {
            let authentication = control.store.with_revision(&document, || {
                document.authenticate_signing(&id, &password, &job)
            })?;
            Ok(PreparedSign {
                document,
                id,
                authentication,
                epoch,
                job,
            })
        })
        .await
    }
    fn finish_sign(
        &self,
        core: &AppCore,
        commitment: &ApprovalCommitment,
        prepared: PreparedSign,
        binding: transaction_policy::ApprovalBindingInput<'_>,
        cancelled: impl Fn() -> bool,
    ) -> Result<WalletSigned, IpcError> {
        let started = unix_now()?;
        let PreparedSign {
            document,
            id,
            authentication,
            epoch,
            job,
        } = prepared;
        let mut outcome = None;
        let owned = self.store.with_revision(&document, || {
            let mut state = self.state.lock().map_err(|_| Error::Cancelled)?;
            if self.revoked.load(Ordering::Acquire)
                || job.check().is_err()
                || state.selected.as_ref() != Some(&id)
                || state.document.as_ref() != Some(&document)
                || !state
                    .opened
                    .as_ref()
                    .is_some_and(|o| o.matches(&document).unwrap_or(false))
            {
                state.lock();
                return Err(Error::Cancelled);
            }
            let session = state.session.as_mut().ok_or(Error::AuthenticationFailed)?;
            let result =
                authentication.sign(&document, &job, commitment, session, core, started, || {
                    EvmSigningObservation::new(
                        binding,
                        unix_now().unwrap_or_default(),
                        Instant::now(),
                        job.check().is_ok()
                            && !self.revoked.load(Ordering::Acquire)
                            && native::application_is_active_after_authorization()
                            && !cancelled(),
                    )
                });
            if result.is_ok() && job.check().is_ok() && !self.revoked.load(Ordering::Acquire) {
                state.last_activity = Some(Instant::now());
            } else {
                state.lock();
            }
            outcome = Some(result);
            Ok(())
        });
        owned.map_err(password_error)?;
        if self.revoked.load(Ordering::Acquire) || job.check().is_err() || cancelled() {
            return Err(cancelled_error());
        }
        let signature = outcome
            .ok_or_else(IpcError::internal)?
            .map_err(|e| wallet_error(&e.to_string(), false))?;
        Ok(WalletSigned {
            signature,
            submission: WalletSubmission::new(
                self.clone(),
                document,
                id,
                epoch,
                binding.expires_at_unix,
            ),
        })
    }
    pub(crate) async fn sign_exact_transaction(
        &self,
        core: &AppCore,
        commitment: &ApprovalCommitment,
        evidence: &crate::evm_approval::presentation::ApprovalEvidence,
        cancellation: &super::TransferCancellation,
        revalidate: impl std::future::Future<Output = Result<(), IpcError>>,
    ) -> Result<WalletSigned, IpcError> {
        let result = match self.prepare_sign().await {
            Ok(prepared) => match revalidate.await {
                Ok(()) => {
                    self.finish_sign(core, commitment, prepared, evidence.binding_input(), || {
                        cancellation.is_cancelled()
                    })
                }
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };
        if result.is_err() {
            let _ = commitment.invalidate(
                ApprovalInvalidationReason::Cancelled,
                unix_now().unwrap_or_default(),
                core,
            );
        }
        result
    }
    pub(crate) async fn sign_exact_x402_payment(
        &self,
        core: &AppCore,
        commitment: &ApprovalCommitment,
        approval: &x402_client::PreparedX402Approval,
        cancelled: impl Fn() -> bool,
    ) -> Result<WalletSigned, IpcError> {
        let binding = approval.binding_input().map_err(|_| IpcError::internal())?;
        let result = if cancelled() {
            Err(cancelled_error())
        } else {
            match self.prepare_sign().await {
                Ok(_) if cancelled() => Err(cancelled_error()),
                Ok(prepared) => self.finish_sign(core, commitment, prepared, binding, cancelled),
                Err(error) => Err(error),
            }
        };
        if result.is_err() {
            let _ = commitment.invalidate(
                ApprovalInvalidationReason::Cancelled,
                unix_now().unwrap_or_default(),
                core,
            );
        }
        result
    }
}

pub(super) fn unix_now() -> Result<u64, IpcError> {
    u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp())
        .map_err(|_| IpcError::internal())
}
