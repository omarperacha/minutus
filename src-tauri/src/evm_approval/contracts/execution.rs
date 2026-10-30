//! One immutable user-scoped route; no generic raw-body signing endpoint exists.
use super::{
    observation::{CompletedStep, complete_facts, prerequisites, residual_facts, wait_and_verify},
    preparation::{Context, ContractWorkflowRequest},
    signing::review_and_send,
};
use crate::{
    evm_approval::EvmApprovalControl,
    wallet_lifecycle::{TransferCancellation, WalletControl},
};
use app_core::AppCore;
use ipc_types::{Conversation, EvmApprovalDecision, IpcError, MessageRole};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::sync::{Arc, atomic::AtomicBool};
use tauri::Manager as _;

pub(crate) fn execute(
    app: &tauri::AppHandle,
    conversation: &Conversation,
    request: &ContractWorkflowRequest,
    cancelled: &Arc<AtomicBool>,
) -> Result<Value, IpcError> {
    request.validate()?;
    // Packaged worker admission is mandatory even before model advertisement;
    // a forged tool invocation must independently face the same host check.
    if !super::available() {
        return Err(IpcError::permission_denied(
            "Packaged isolated contract qualification is unavailable",
        ));
    }
    let core = app.state::<Arc<AppCore>>();
    let wallet = app.state::<WalletControl>();
    let approval = app.state::<EvmApprovalControl>();
    let _lease = approval.begin_reconciliation()?;
    wallet.transfer_work(app, cancelled, None, |cancel| {
        tauri::async_runtime::block_on(run(
            app,
            core.inner(),
            wallet.inner(),
            conversation,
            request,
            cancelled,
            cancel,
        ))
    })
}
async fn run(
    app: &tauri::AppHandle,
    core: &Arc<AppCore>,
    wallet: &WalletControl,
    conversation: &Conversation,
    request: &ContractWorkflowRequest,
    cancelled: &Arc<AtomicBool>,
    cancel: &TransferCancellation,
) -> Result<Value, IpcError> {
    let account = super::readiness::initial_account(&wallet.public_state()?)?;
    cancel.check()?;
    let scope = request.scope(&account)?;
    let message = conversation
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .ok_or_else(IpcError::internal)?;
    let Some(session) = core
        .start_contract_workflow(&conversation.id, &message.id, scope.clone())
        .map_err(crate::error_mapping::app_core_error)?
    else {
        return Ok(json!({"state":"alreadyAttempted","retryAutomatically":false}));
    };
    let endpoint = Arc::new(crate::evm_rpc_settings::resolved(
        crate::evm_rpc_settings::chain_network(request.network),
    )?);
    let scope_decision = super::scope_review::review(app, &scope, &endpoint, request).await?;
    if scope_decision != EvmApprovalDecision::Approved {
        if let Some(work) = session
            .claim_next(vec![])
            .map_err(crate::error_mapping::app_core_error)?
        {
            let _ = work.stop_before_send(if scope_decision == EvmApprovalDecision::Denied {
                app_core::ContractStepState::Denied
            } else {
                app_core::ContractStepState::Cancelled
            });
        }
        return Ok(json!({"state":"scopeDenied","retryAutomatically":false}));
    }
    cancel.check()?;
    let origin = crate::evm_approval::ApprovalOrigin::Chat {
        conversation: Sha256::digest(conversation.id.0.as_bytes()).into(),
        call: Sha256::digest(format!("{}:{}", message.id.0, scope.commitment()).as_bytes()).into(),
    };
    let mut context = Context::new(
        request,
        account.clone(),
        0,
        Arc::clone(cancelled),
        origin,
        endpoint,
    )
    .await?;
    cancel.check()?;
    let ready = wallet.ensure_account_ready(core).await?;
    if !ready.eq_ignore_ascii_case(&account) {
        return Err(IpcError::invalid_request(
            "The selected account changed after source preflight",
        ));
    }
    context.wallet_epoch = wallet.transfer_epoch()?;
    let context = Arc::new(context);
    run_steps(app, core, wallet, &session, &context, cancel).await
}
async fn run_steps(
    app: &tauri::AppHandle,
    core: &Arc<AppCore>,
    wallet: &WalletControl,
    session: &app_core::ContractWorkflowSession,
    context: &Arc<Context>,
    cancel: &TransferCancellation,
) -> Result<Value, IpcError> {
    let scope = session.scope();
    let mut completed: Vec<CompletedStep> = Vec::new();
    let result=async {
    while completed.len() < scope.intent().steps.len() {
        context.check(wallet, cancel)?;
        let proofs = prerequisites(context, &completed).await?;
        context.check(wallet, cancel)?;
        let Some(work) = session
            .claim_next(proofs)
            .map_err(crate::error_mapping::app_core_error)?
        else {
            return Err(IpcError::invalid_request(
                "The workflow cannot advance; reconcile the recorded step before a deliberate new request",
            ));
        };
        let candidate = match context.prepare(work.intent()).await {
            Ok(candidate) => candidate,
            Err(error) => {
                let _ = work.stop_before_send(app_core::ContractStepState::Failed);
                return Err(error);
            }
        };
        context.check(wallet, cancel)?;
        let step = Arc::new(
            work.bind_candidate(candidate)
                .map_err(crate::error_mapping::app_core_error)?,
        );
        let record = match review_and_send(app, core, wallet, context, &step, &completed, cancel)
            .await
        {
            Ok(Some(record)) => record,
            Ok(None) => {
                return Ok(
                    json!({"state":"deniedOrDismissed","completedSteps":completed.len(),"residualAuthority":residual_facts(&completed),"retryAutomatically":false}),
                );
            }
            Err(error) => {
                let _ = step.stop_before_send(app_core::ContractStepState::Failed);
                return Err(error);
            }
        };
        completed.push(wait_and_verify(context, wallet, cancel, core, step, record).await?);
    }
    let proof_report=complete_facts(&completed)?;
    Ok(
        json!({"state":"verified","transactions":completed.iter().map(|step|&step.hash).collect::<Vec<_>>(),"scopeCommitment":scope.commitment(),"verifiedPostconditions":proof_report.postconditions,"residualAuthority":proof_report.residual,"retryAutomatically":false,"qualification":"Exact compiled profile, transaction and receipt inclusion, and fresh ordered predicates verified at the listed blocks; these are historical observations, not finality or a claim about subsequent current state."}),
    )
    }.await;
    match result {
        Ok(value) => Ok(value),
        Err(mut error) => {
            error.message=json!({"state":"stopped","message":error.message,"completedSteps":completed.len(),"residualAuthority":residual_facts(&completed),"retryAutomatically":false}).to_string();
            error.retryable = false;
            Err(error)
        }
    }
}
