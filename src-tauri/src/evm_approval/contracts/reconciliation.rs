//! Public observation of one previously dispatched hash; no wallet or send path.
use crate::{
    evm_approval::{
        EvmApprovalControl,
        execution::show_native_review,
        presentation::{ApprovalDisplay, ReviewKind},
    },
    wallet_lifecycle::TransferCancellation,
};
use app_core::{AppCore, ContractReconciliation};
use chain_evm::{
    EvmNetwork, EvmRpcUrl, TransactionHash, TransactionLifecycle,
    contract::verify_submitted_transaction, monitor_transaction_bounded,
};
use ipc_types::{Conversation, EvmApprovalDecision, IpcError};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tauri::Manager as _;

pub(crate) fn execute(
    app: &tauri::AppHandle,
    conversation: &Conversation,
    hash: &str,
    cancelled: &Arc<AtomicBool>,
) -> Result<Value, IpcError> {
    let hash = TransactionHash::new(hash)
        .map_err(|_| IpcError::invalid_request("Expected one exact transaction hash"))?;
    let core = app.state::<Arc<AppCore>>();
    let control = app.state::<EvmApprovalControl>();
    let _lease = control.begin_reconciliation()?;
    let context = core
        .contract_reconciliation(&conversation.id, hash.as_str())
        .map_err(crate::error_mapping::app_core_error)?
        .ok_or_else(|| {
            IpcError::permission_denied(
                "This conversation has no retained dispatched workflow for that hash",
            )
        })?;
    let network = crate::evm_transactions::ipc_network(&context.declaration().network)?;
    let chain = crate::evm_rpc_settings::chain_network(network);
    let endpoint = crate::evm_rpc_settings::resolved(chain)?;
    let cancellation = TransferCancellation::new(Arc::clone(cancelled));
    cancellation.check()?;
    let display = consent_display(&context, &endpoint, chain);
    let decision = tauri::async_runtime::block_on(show_native_review(app, display, &cancellation))?;
    if decision != EvmApprovalDecision::Approved {
        return Err(IpcError::permission_denied(
            "Read-only reconciliation was declined; no transaction action was taken",
        ));
    }
    cancellation.check()?;
    let observed = monitor_transaction_bounded(
        chain,
        &endpoint,
        &hash,
        &context.declaration().account,
        context.nonce(),
        Duration::from_secs(30),
        cancelled,
    );
    cancellation.check()?;
    let observation = match observed {
        Ok(observation) => observation,
        Err(failure) => {
            return Ok(
                json!({"state":"verificationUnavailable","transactionHash":hash.as_str(),"monitorFailure":failure,"execution":"unknown","workflowCompletion":"unverified","writesResumed":false,"retryAutomatically":false}),
            );
        }
    };
    if observation.lifecycle() == TransactionLifecycle::Pending {
        return Ok(
            json!({"state":"pending","transactionHash":hash.as_str(),"execution":"notObserved","workflowCompletion":"unverified","writesResumed":false,"retryAutomatically":false}),
        );
    }
    // A raw missing-transaction/advanced-nonce claim is not inclusion evidence.
    if observation.lifecycle() == TransactionLifecycle::DroppedOrReplaced {
        return Ok(
            json!({"state":"notObserved","transactionHash":hash.as_str(),"providerObservation":"transaction absent and sender nonce advanced","execution":"unknown","workflowCompletion":"unverified","writesResumed":false,"retryAutomatically":false}),
        );
    }
    let proof = match verify_submitted_transaction(
        chain,
        &endpoint,
        hash.as_str(),
        &context.declaration().account,
        context.nonce(),
        context.signing_digest(),
        cancelled,
    ) {
        Ok(proof) => proof,
        Err(failure) => {
            return Ok(
                json!({"state":"verificationUnavailable","transactionHash":hash.as_str(),"inclusionProof":"unavailable","inclusionFailure":failure.to_string(),"execution":"unknown","workflowCompletion":"unverified","writesResumed":false,"retryAutomatically":false}),
            );
        }
    };
    cancellation.check()?;
    let value = json!({"state":if proof.succeeded(){"executionConfirmed"}else{"executionReverted"},
        "transactionHash":proof.transaction_hash(),"stepIndex":context.step_index(),
        "receiptBlockNumber":proof.block_number().to_string(),"receiptBlockHash":proof.block_hash(),
        "inclusionCommitment":proof.inclusion_commitment(),"canonicalAtObservation":true,
        "workflowCompletion":"notReverified","currentGoals":"notRechecked",
        "qualification":"Exact stored signed body, account, nonce, network and canonical transaction/receipt inclusion were checked. Receipt status does not prove declared source/effects, completion conditions, current goals, causation or finality. Previously recorded completion evidence remains historical only.",
        "writesResumed":false,"retryAutomatically":false,"automaticCleanup":false});
    core.apply_contract_reconciliation(
        &context,
        proof,
        &crate::error_mapping::current_timestamp()?,
    )
    .map_err(crate::error_mapping::app_core_error)?;
    Ok(value)
}

fn consent_display(
    context: &ContractReconciliation,
    endpoint: &EvmRpcUrl,
    chain: EvmNetwork,
) -> ApprovalDisplay {
    ApprovalDisplay {
        kind: ReviewKind::OutboundPermission,
        title: "Allow transaction reconciliation?".into(),
        body: format!(
            "READ-ONLY TRANSACTION RECONCILIATION — NO TRANSACTION APPROVAL\n\nNetwork: {}\nProvider origin: {}\nKnown transaction: {}\nOriginal account: {}\nOriginal nonce: {}\n\nAllow bounded status lookups, canonical block headers and full public block transaction/receipt data needed to prove this exact transaction's inclusion. The provider sees the displayed public hash/account and related public block queries. No source or compiler service is contacted.\n\nThis cannot unlock a wallet, request a password, sign, submit, replace or retry a transaction, reopen an attempt or advance another write. Receipt inclusion does not establish source/effect qualification or declared completion/current goals. Existing workflow authority remains retired.",
            chain.descriptor().display_name(),
            endpoint.public_origin(),
            context.transaction_hash(),
            context.declaration().account,
            context.nonce()
        ),
        technical_details: String::new(),
        model_summary: None,
        final_disclaimer: String::new(),
        has_warnings: false,
        approval_allowed: true,
    }
}
