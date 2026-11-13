//! One generic declared workflow preserves native authority at every write.
use super::{
    context::Context,
    lifecycle::Lifecycle,
    observation::{CompletedStep, ObservationFailure, wait_and_verify},
    reporting,
    request::DeclaredWorkflowRequest,
};
use crate::{
    evm_approval::EvmApprovalControl,
    wallet_lifecycle::{TransferCancellation, WalletControl},
};
use app_core::{AppCore, ContractStepState, DeclaredContractWorkflowSession};
use chain_evm::contract::generic::verify_generic_goals;
use ipc_types::{Conversation, EvmApprovalDecision, IpcError, MessageRole};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, atomic::AtomicBool};
use tauri::Manager as _;

pub(crate) fn available() -> bool {
    super::super::available() && solidity_compilation::TrustedCompiler::packaged().is_ok()
}
pub(crate) fn execute(
    app: &tauri::AppHandle,
    conversation: &Conversation,
    request: &DeclaredWorkflowRequest,
    cancelled: &Arc<AtomicBool>,
) -> Result<Value, IpcError> {
    request.validate()?;
    if !available() {
        return Err(IpcError::permission_denied(
            "Packaged generic source and execution qualification is unavailable",
        ));
    }
    let core = app.state::<Arc<AppCore>>();
    let wallet = app.state::<WalletControl>();
    let approval = app.state::<EvmApprovalControl>();
    let _lease = approval.begin_reconciliation()?;
    let mut lifecycle = Lifecycle::default();
    let outcome = wallet.transfer_work(app, cancelled, None, |cancel| {
        Ok(tauri::async_runtime::block_on(run(
            app,
            core.inner(),
            wallet.inner(),
            conversation,
            request,
            cancelled,
            cancel,
            &mut lifecycle,
        )))
    });
    match outcome {
        Ok(Ok(value)) => Ok(lifecycle.project_success(value)),
        Ok(Err(failure)) => Err(lifecycle.project_error(
            failure.error,
            failure.completed_steps,
            &failure.partial_facts,
        )),
        Err(error) => Err(lifecycle.project_error(error, 0, &Value::Null)),
    }
}
struct Failure {
    error: IpcError,
    completed_steps: usize,
    partial_facts: Value,
}
impl From<IpcError> for Failure {
    fn from(error: IpcError) -> Self {
        Self {
            error,
            completed_steps: 0,
            partial_facts: Value::Null,
        }
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "Native workflow owners and request-local reporting stay explicit"
)]
async fn run(
    app: &tauri::AppHandle,
    core: &Arc<AppCore>,
    wallet: &WalletControl,
    conversation: &Conversation,
    request: &DeclaredWorkflowRequest,
    cancelled: &Arc<AtomicBool>,
    cancel: &TransferCancellation,
    lifecycle: &mut Lifecycle,
) -> Result<Value, Failure> {
    cancel.check()?;
    let message = conversation
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .ok_or_else(IpcError::internal)?;
    let Some(attempt) = core
        .claim_contract_workflow_attempt(&conversation.id, &message.id)
        .map_err(crate::error_mapping::app_core_error)?
    else {
        return Ok(json!({"state":"alreadyAttempted","retryAutomatically":false}));
    };
    lifecycle.wallet_readiness_requested(wallet.public_state()?.availability);
    let account =
        super::super::readiness::initial_account(cancel, wallet.ensure_account_ready(core)).await?;
    lifecycle.wallet_ready();
    let declaration = request.declaration(&account)?;
    let Some(session) = attempt
        .start_declared(declaration.clone())
        .map_err(crate::error_mapping::app_core_error)?
    else {
        return Ok(json!({"state":"alreadyAttempted","retryAutomatically":false}));
    };
    let endpoint = Arc::new(crate::evm_rpc_settings::resolved(
        crate::evm_rpc_settings::chain_network(request.network),
    )?);
    lifecycle.scope_review_requested();
    let decision = super::scope_review::review(app, &declaration, &endpoint).await?;
    lifecycle.scope_decision(decision);
    if decision != EvmApprovalDecision::Approved {
        stop_initial(
            &session,
            if decision == EvmApprovalDecision::Denied {
                ContractStepState::Denied
            } else {
                ContractStepState::Cancelled
            },
        )?;
        return Ok(json!({"state":"scopeDenied","retryAutomatically":false}));
    }
    cancel.check()?;
    let origin = crate::evm_approval::ApprovalOrigin::Chat {
        conversation: Sha256::digest(conversation.id.0.as_bytes()).into(),
        call: Sha256::digest(format!("{}:{}", message.id.0, declaration.commitment()).as_bytes())
            .into(),
    };
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| IpcError::internal())?
        .join("public-contract-sources");
    lifecycle.source_preflight_started();
    let mut context = match Context::preflight(
        &declaration,
        endpoint,
        Arc::clone(cancelled),
        directory,
        origin,
    )
    .await
    {
        Ok(context) => context,
        Err(error) => {
            stop_initial(&session, ContractStepState::Failed)?;
            return Err(error.into());
        }
    };
    lifecycle.source_preflight_succeeded();
    cancel.check()?;
    let ready = wallet.ensure_account_ready(core).await?;
    if !ready.eq_ignore_ascii_case(&account) {
        stop_initial(&session, ContractStepState::Cancelled)?;
        return Err(IpcError::invalid_request(
            "The selected account changed after source preflight",
        )
        .into());
    }
    context.wallet_epoch = wallet.transfer_epoch()?;
    run_steps(
        app,
        core,
        wallet,
        &session,
        request,
        &Arc::new(context),
        cancel,
        lifecycle,
    )
    .await
}
fn stop_initial(
    session: &DeclaredContractWorkflowSession,
    state: ContractStepState,
) -> Result<(), IpcError> {
    if let Some(step) = session
        .claim_next()
        .map_err(crate::error_mapping::app_core_error)?
    {
        step.stop_before_send(state)
            .map_err(crate::error_mapping::app_core_error)?;
    }
    Ok(())
}
#[expect(
    clippy::too_many_arguments,
    reason = "Immutable model presentation stays separate from persisted workflow authority"
)]
async fn run_steps(
    app: &tauri::AppHandle,
    core: &Arc<AppCore>,
    wallet: &WalletControl,
    session: &DeclaredContractWorkflowSession,
    request: &DeclaredWorkflowRequest,
    context: &Arc<Context>,
    cancel: &TransferCancellation,
    lifecycle: &mut Lifecycle,
) -> Result<Value, Failure> {
    let mut completed = Vec::<CompletedStep>::new();
    let result=async{
        while completed.len()<session.declaration().intent().steps.len(){
            context.check(wallet,cancel)?;
            let work=session.claim_next().map_err(crate::error_mapping::app_core_error)?.ok_or_else(||IpcError::invalid_request("The declared workflow cannot advance; reconcile its recorded state before a deliberate new request"))?;
            let previous=completed.iter().map(|step|Arc::clone(&step.completion)).collect::<Vec<_>>();
            let (candidate,proof)=match context.prepare(&work,&previous).await{Ok(result)=>result,Err(error)=>{let _=work.stop_before_send(ContractStepState::Failed);return Err(error);}};
            context.check(wallet,cancel)?;
            let step=Arc::new(work.bind_generic_candidate(candidate,proof).map_err(crate::error_mapping::app_core_error)?);
            context.remember(step.shared_candidate())?;
            let progress=lifecycle.step(usize::from(step.index())).ok_or_else(IpcError::internal)?;
            let record=match super::signing::review_and_send(app,core,wallet,context,&step,&completed,cancel,progress,request.summary(step.index())).await{Ok(Some(record))=>record,Ok(None)=>return Ok(json!({"state":"deniedOrDismissed","completedSteps":completed.len(),"partialFacts":reporting::partial(context,&completed),"retryAutomatically":false})),Err(error)=>{let _=step.stop_before_send(ContractStepState::Failed);return Err(error);}};
            completed.push(wait_and_verify(context,wallet,cancel,core,step,record).await?);
        }
        if session.declaration().intent().goals.is_empty() {
            return Ok(json!({"state":"executionConfirmed","goalsDeclared":false,"goalStatus":"undeclared; exact included execution does not establish a business/task outcome","scopeCommitment":session.declaration().commitment(),"partialFacts":reporting::partial(context,&completed),"retryAutomatically":false,"automaticCleanup":false}));
        }
        let goals=match final_goals(context,session,&completed).await {
            Ok(goals) => goals,
            Err(ObservationFailure::Unsatisfied) => return Ok(json!({"state":"executionConfirmed","goalsDeclared":true,"goalStatus":"unsatisfied at proof-checked observation","partialFacts":reporting::partial(context,&completed),"retryAutomatically":false,"automaticCleanup":false})),
            Err(ObservationFailure::Unknown(error)) => return Err(error),
        };context.check(wallet,cancel)?;
        context.record_conditions(goals.conditions())?;
        let current_goals=session.apply_goals(goals).map_err(crate::error_mapping::app_core_error)?;
        Ok(json!({"state":"verified","scopeCommitment":session.declaration().commitment(),"historicalCompletions":reporting::completions(&completed),"verifiedCurrentGoals":current_goals,"residualState":reporting::partial(context,&completed)["residualState"],"qualification":"Exact source/ABI, complete structural execution effects, canonical successful inclusions and separately declared state conditions were independently checked. Source labels are not economic semantics; observations are historical and do not establish finality or later current state.","retryAutomatically":false,"automaticCleanup":false}))
    }.await;
    match result {
        Ok(value) => Ok(value),
        Err(error) => Err(Failure {
            error,
            completed_steps: completed.len(),
            partial_facts: reporting::partial(context, &completed),
        }),
    }
}
async fn final_goals(
    context: &Context,
    session: &DeclaredContractWorkflowSession,
    completed: &[CompletedStep],
) -> Result<chain_evm::contract::generic::VerifiedGenericGoals, ObservationFailure> {
    let endpoint = Arc::clone(&context.endpoint);
    let network = context.network;
    let cancelled = Arc::clone(&context.cancelled);
    let identities = context.identity_views();
    let candidates = completed
        .iter()
        .map(|step| step.step.shared_candidate())
        .collect::<Vec<_>>();
    let completed = completed
        .iter()
        .map(|step| Arc::clone(&step.completion))
        .collect::<Vec<_>>();
    let conditions = session
        .goals()
        .map_err(|error| crate::error_mapping::app_core_error(error.into()))?;
    tokio::task::spawn_blocking(move || {
        let mut identities = identities.iter().map(Arc::as_ref).collect::<Vec<_>>();
        for candidate in &candidates {
            if !identities
                .iter()
                .any(|identity| identity.commitment() == candidate.identity().commitment())
            {
                identities.push(candidate.identity());
            }
        }
        let historical = completed
            .iter()
            .map(|step| step.proof())
            .collect::<Vec<_>>();
        verify_generic_goals(
            &endpoint,
            network,
            &identities,
            &historical,
            &conditions,
            &cancelled,
        )
    })
    .await
    .map_err(|_| ObservationFailure::Unknown(IpcError::internal()))?
    .map_err(ObservationFailure::from)
}
