//! One-shot chat admission and composition into the existing native approval owner.
use super::super::filesystem::ipc_json_value;
use super::intent::{Asset, intent};
use crate::{
    evm_approval::{ApprovalCandidate, ApprovalOrigin, EvmApprovalControl, review_chat_transfer},
    evm_rpc_settings::chain_network,
    evm_transfers::preparation,
    wallet_lifecycle::WalletControl,
};
use app_core::AppCore;
use ipc_types::{
    Conversation, EvmApprovalDecision, IpcError, IpcErrorCode, MessageRole, ToolOutcome,
    ToolResultPart,
};
use model_runtime::ParsedToolCall;
use sha2::{Digest as _, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tauri::Manager as _;
pub(crate) fn execute(
    app: &tauri::AppHandle,
    core: &AppCore,
    conversation: &Conversation,
    call: &ParsedToolCall,
    cancelled: &Arc<AtomicBool>,
) -> Result<ToolOutcome, IpcError> {
    let intent = intent(call)?;
    if cancelled.load(Ordering::Acquire) {
        return outcome(&serde_json::json!({"state":"cancelled","retryAutomatically":false}));
    }
    let message = conversation
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .ok_or_else(IpcError::internal)?;
    if !core
        .claim_chat_transfer(&conversation.id, &message.id)
        .map_err(crate::error_mapping::app_core_error)?
    {
        return outcome(
            &serde_json::json!({"state":"alreadyAttempted","retryAutomatically":false,"guidance":"This user turn already attempted a transfer, or the turn is not valid. No second transfer was prepared. Check Wallet history; a deliberate new request requires a new user message."}),
        );
    }
    let wallet = app.state::<WalletControl>();
    let result = wallet.transfer_work(app, cancelled, None, |aborted| tauri::async_runtime::block_on(async {
        let sender = wallet.ensure_account_ready(core).await?;
        if aborted.is_cancelled() { return Err(IpcError { code:IpcErrorCode::Cancelled, message:"Transfer cancelled during unlock".into(),retryable:false }); }
        let epoch = wallet.transfer_epoch()?;
        let network = chain_network(intent.network);
        let approval=app.state::<EvmApprovalControl>();
        crate::evm_approval::reconcile_prior_transfers(app,core,&approval,&wallet,network,aborted).await?;
        let candidate = match intent.asset {
            Asset::Eth => ApprovalCandidate::Native(
                preparation::native(network, &sender, &intent.recipient, &intent.amount).await?,
            ),
            Asset::Usdc => ApprovalCandidate::Token(
                preparation::token(network, &sender, &intent.recipient, &intent.amount).await?,
            ),
        };
        let simulation = match &candidate { ApprovalCandidate::Native(c) => c.simulation(), ApprovalCandidate::Token(c) => c.simulation() };
        if !preparation::RefreshPolicy::Chat.accepts(simulation.state(), simulation.state()) {
            return Err(IpcError::invalid_request("Chat transfers require successful exact-block simulation. ETH contract recipients are outside chat Phase A; use Wallet for its existing explicit warned review."));
        }
        wallet.check_transfer_epoch(epoch)?;
        if cancelled.load(Ordering::Acquire) || aborted.is_cancelled() {
            return Err(IpcError {
                code: IpcErrorCode::Cancelled,
                message: "Transfer cancelled before review".into(),
                retryable: false,
            });
        }
        let approval = app.state::<EvmApprovalControl>();
        let origin = ApprovalOrigin::Chat {
            conversation: Sha256::digest(conversation.id.0.as_bytes()).into(),
            call: Sha256::digest(call.call_id.as_bytes()).into(),
        };
        let review_id = approval.register(candidate, origin, epoch)?;
        let managed_core = app.state::<Arc<AppCore>>();
        // Already on the synchronous tool worker; the shared review owns its runtime bridge.
        Ok((
            review_id,
            wallet.inner().clone(),
            managed_core.inner().clone(),
        ))
    }));
    let (review_id, wallet, managed_core) = match result {
        Ok(prepared) => prepared,
        Err(error) => return failed_outcome(&error, FailurePhase::Preparation),
    };
    let approval = app.state::<EvmApprovalControl>();
    match review_chat_transfer(app, &managed_core, &approval, &wallet, review_id, cancelled) {
        Ok(reviewed) => project_review(
            core,
            &reviewed.outcome,
            reviewed.submission_uncertain,
            &intent,
        ),
        Err(error) => failed_outcome(&error, FailurePhase::Review),
    }
}
#[derive(Clone, Copy)]
enum FailurePhase {
    Preparation,
    Review,
}
fn failed_outcome(error: &IpcError, phase: FailurePhase) -> Result<ToolOutcome, IpcError> {
    let after_preparation = matches!(phase, FailurePhase::Review);
    outcome(&serde_json::json!({
        "state": if error.code == IpcErrorCode::Cancelled { "cancelled" }
            else if error.message.contains("expired") { "expired" }
            else if after_preparation { "reviewFailed" } else { "preparationFailed" },
        "phase": if after_preparation { "reviewOrSigning" } else { "preparation" },
        "approvalMayHaveBeenRequested": after_preparation,
        "message": error.message,
        "retryAutomatically": false,
        "guidance": if after_preparation {
            "The native review or signing flow failed after preparation. Do not claim that no approval was requested, invent wallet setup or account-authorisation instructions, or retry automatically. Report the host error; a different-signing-account error requires application investigation."
        } else {
            "Preparation failed before transaction approval. Report the host error without inventing wallet setup instructions."
        }
    }))
}
#[cfg(test)]
#[path = "execution_failure_tests.rs"]
mod failure_tests;
fn outcome(value: &serde_json::Value) -> Result<ToolOutcome, IpcError> {
    Ok(ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: ipc_json_value(value)?,
        }],
    })
}

fn project_review(
    core: &AppCore,
    review: &ipc_types::EvmApprovalReviewOutcome,
    uncertain: bool,
    intent: &super::intent::Intent,
) -> Result<ToolOutcome, IpcError> {
    let observed = if uncertain {
        None
    } else {
        review.transaction_hash.as_ref().and_then(|hash| {
            core.evm_transactions()
                .ok()?
                .into_iter()
                .find(|record| &record.transaction_hash == hash)
        })
    };
    let monitored = observed.and_then(|record| {
        tauri::async_runtime::block_on(crate::evm_transactions::refresh_record(core, record)).ok()
    });
    let confirmed = monitored.as_ref().is_some_and(|record| {
        record.status_available && record.lifecycle == ipc_types::EvmTransactionLifecycle::Confirmed
    });
    let reverted = monitored.as_ref().is_some_and(|record| {
        record.status_available && record.lifecycle == ipc_types::EvmTransactionLifecycle::Failed
    });
    outcome(&serde_json::json!({
        "state": match review.decision {
            EvmApprovalDecision::Denied => "denied",
            EvmApprovalDecision::Dismissed => "cancelled",
            EvmApprovalDecision::Approved if uncertain => "submissionUncertain",
            EvmApprovalDecision::Approved if confirmed => "confirmed",
            EvmApprovalDecision::Approved if reverted => "reverted",
            EvmApprovalDecision::Approved => "submitted",
        },
        "network":intent.network,"asset": match intent.asset {Asset::Eth=>"ETH",Asset::Usdc=>"USDC"},
        "transactionHash":review.transaction_hash,"explorerUrl":review.explorer_url,
        "confirmed":confirmed,"retryAutomatically":false,
        "guidance":"Submitted is pending, not confirmation. Inspect transaction status in Wallet; never automatically repeat a denied, cancelled or uncertain transfer."
    }))
}
