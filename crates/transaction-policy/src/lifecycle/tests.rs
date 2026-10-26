use super::*;

const CANDIDATE: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TRANSACTION_HASH: &str = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn simulated_machine() -> Result<TransactionStateMachine, TransactionLifecycleError> {
    let mut machine = TransactionStateMachine::propose("transaction-1", CANDIDATE)?;
    let token = machine.validate(&machine.transition_token())?;
    machine.simulate(&token, CANDIDATE)?;
    Ok(machine)
}

fn broadcast_machine() -> Result<TransactionStateMachine, TransactionLifecycleError> {
    let mut machine = simulated_machine()?;
    let token = machine.await_approval(&machine.transition_token())?;
    let token = machine.approve(&token)?;
    let token = machine.sign(&token)?;
    machine.broadcast(&token, TRANSACTION_HASH)?;
    Ok(machine)
}

#[test]
fn complete_lifecycle_uses_every_required_state_in_order() -> Result<(), Box<dyn std::error::Error>>
{
    let mut machine = TransactionStateMachine::propose("transaction-1", CANDIDATE)?;
    let token = machine.validate(&machine.transition_token())?;
    let token = machine.simulate(&token, CANDIDATE)?;
    let token = machine.await_approval(&token)?;
    let token = machine.approve(&token)?;
    let token = machine.sign(&token)?;
    let token = machine.broadcast(&token, TRANSACTION_HASH)?;
    let token = machine.confirm(&token, TRANSACTION_HASH)?;

    assert_eq!(machine.state().kind(), TransactionStateKind::Confirmed);
    assert_eq!(
        machine.state().broadcast_reference(),
        Some(TRANSACTION_HASH)
    );
    assert_eq!(token.revision(), 7);
    assert_eq!(machine.snapshot().transitions().len(), 8);
    assert_eq!(
        machine
            .snapshot()
            .transitions()
            .iter()
            .map(|transition| transition.state().kind())
            .collect::<Vec<_>>(),
        vec![
            TransactionStateKind::Proposed,
            TransactionStateKind::Validated,
            TransactionStateKind::Simulated,
            TransactionStateKind::AwaitingApproval,
            TransactionStateKind::Approved,
            TransactionStateKind::Signed,
            TransactionStateKind::Broadcast,
            TransactionStateKind::Confirmed,
        ]
    );
    Ok(())
}

#[test]
fn stale_skipped_repeated_and_post_terminal_transitions_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let mut machine = TransactionStateMachine::propose("transaction-1", CANDIDATE)?;
    let stale = machine.transition_token();
    let current = machine.validate(&stale)?;

    assert_eq!(
        machine.validate(&stale),
        Err(TransactionLifecycleError::StaleTransition)
    );
    assert!(matches!(
        machine.await_approval(&current),
        Err(TransactionLifecycleError::InvalidTransition { .. })
    ));
    assert_eq!(machine.state().kind(), TransactionStateKind::Validated);

    let token = machine.fail(&current, TransactionFailureReason::SimulationFailed)?;
    assert_eq!(machine.state().kind(), TransactionStateKind::Failed);
    assert_eq!(
        machine.fail(&token, TransactionFailureReason::InternalInvariant),
        Err(TransactionLifecycleError::TerminalState)
    );
    Ok(())
}

#[test]
fn simulation_and_confirmation_require_exact_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let mut machine = TransactionStateMachine::propose("transaction-1", CANDIDATE)?;
    let token = machine.validate(&machine.transition_token())?;
    assert_eq!(
        machine.simulate(&token, "0xchanged"),
        Err(TransactionLifecycleError::MismatchedCandidate)
    );
    assert_eq!(machine.state().kind(), TransactionStateKind::Validated);

    let mut broadcast = broadcast_machine()?;
    assert_eq!(
        broadcast.confirm(&broadcast.transition_token(), "0xother"),
        Err(TransactionLifecycleError::MismatchedBroadcastReference)
    );
    assert_eq!(broadcast.state().kind(), TransactionStateKind::Broadcast);
    Ok(())
}

#[test]
fn approval_decisions_are_typed_terminal_failures() -> Result<(), Box<dyn std::error::Error>> {
    for reason in [
        TransactionFailureReason::ApprovalDenied,
        TransactionFailureReason::ApprovalDismissed,
        TransactionFailureReason::ApprovalCancelled,
        TransactionFailureReason::ApprovalExpired,
    ] {
        let mut machine = simulated_machine()?;
        let token = machine.await_approval(&machine.transition_token())?;
        machine.fail(&token, reason)?;
        assert_eq!(machine.state().failure_reason(), Some(reason));
        assert_eq!(
            machine
                .snapshot()
                .transitions()
                .last()
                .map(TransactionTransition::reason),
            Some(TransactionTransitionReason::Failed(reason))
        );
    }
    Ok(())
}

#[test]
fn failure_reasons_are_scoped_to_the_owning_stage() -> Result<(), Box<dyn std::error::Error>> {
    let mut machine = TransactionStateMachine::propose("transaction-1", CANDIDATE)?;
    assert_eq!(
        machine.fail(
            &machine.transition_token(),
            TransactionFailureReason::SigningFailed,
        ),
        Err(TransactionLifecycleError::InvalidFailureReason {
            state: TransactionStateKind::Proposed,
            reason: TransactionFailureReason::SigningFailed,
        })
    );
    assert_eq!(machine.state().kind(), TransactionStateKind::Proposed);
    Ok(())
}

#[test]
fn every_fallible_stage_has_an_explicit_failed_edge() -> Result<(), Box<dyn std::error::Error>> {
    let proposed = TransactionStateMachine::propose("proposed", CANDIDATE)?;

    let mut validated = TransactionStateMachine::propose("validated", CANDIDATE)?;
    validated.validate(&validated.transition_token())?;

    let simulated = simulated_machine()?;

    let mut awaiting = simulated.clone();
    awaiting.await_approval(&awaiting.transition_token())?;

    let mut approved = awaiting.clone();
    approved.approve(&approved.transition_token())?;

    let mut signed = approved.clone();
    signed.sign(&signed.transition_token())?;

    let broadcast = broadcast_machine()?;

    for (mut machine, reason) in [
        (proposed, TransactionFailureReason::ValidationFailed),
        (validated, TransactionFailureReason::SimulationFailed),
        (
            simulated,
            TransactionFailureReason::ApprovalPreparationFailed,
        ),
        (awaiting, TransactionFailureReason::ApprovalDenied),
        (approved, TransactionFailureReason::SigningFailed),
        (signed, TransactionFailureReason::BroadcastFailed),
        (broadcast, TransactionFailureReason::ConfirmationFailed),
    ] {
        machine.fail(&machine.transition_token(), reason)?;
        assert_eq!(machine.state().failure_reason(), Some(reason));
    }
    Ok(())
}

#[test]
fn restart_invalidates_every_pre_broadcast_state() -> Result<(), Box<dyn std::error::Error>> {
    let mut machines = Vec::new();
    let proposed = TransactionStateMachine::propose("proposed", CANDIDATE)?;
    machines.push(proposed);

    let mut validated = TransactionStateMachine::propose("validated", CANDIDATE)?;
    validated.validate(&validated.transition_token())?;
    machines.push(validated);

    let simulated = simulated_machine()?;
    machines.push(simulated.clone());

    let mut awaiting = simulated.clone();
    awaiting.await_approval(&awaiting.transition_token())?;
    machines.push(awaiting.clone());

    let mut approved = awaiting;
    approved.approve(&approved.transition_token())?;
    machines.push(approved.clone());

    let mut signed = approved;
    signed.sign(&signed.transition_token())?;
    machines.push(signed);

    for machine in machines {
        let restored = TransactionStateMachine::restore(machine.snapshot(), None)?;
        assert_eq!(
            restored.state().failure_reason(),
            Some(TransactionFailureReason::ApplicationRestarted)
        );
    }
    Ok(())
}

#[test]
fn restart_resumes_broadcast_only_with_exact_public_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let machine = broadcast_machine()?;
    let snapshot = machine.snapshot();
    assert_eq!(
        TransactionStateMachine::restore(snapshot.clone(), None),
        Err(TransactionLifecycleError::MissingRestartEvidence)
    );
    let wrong = RestartEvidence::broadcast(CANDIDATE, "0xwrong")?;
    assert_eq!(
        TransactionStateMachine::restore(snapshot.clone(), Some(&wrong)),
        Err(TransactionLifecycleError::MismatchedRestartEvidence)
    );
    let exact = RestartEvidence::broadcast(CANDIDATE, TRANSACTION_HASH)?;
    let restored = TransactionStateMachine::restore(snapshot, Some(&exact))?;
    assert_eq!(restored.state().kind(), TransactionStateKind::Broadcast);
    assert_eq!(restored.transition_token().revision(), 6);
    Ok(())
}

#[test]
fn snapshot_validation_rejects_tampered_history() -> Result<(), Box<dyn std::error::Error>> {
    let machine = simulated_machine()?;
    let mut snapshot = machine.snapshot();
    snapshot.transitions[1].from = Some(TransactionStateKind::Approved);
    assert!(matches!(
        TransactionStateMachine::restore(snapshot, None),
        Err(TransactionLifecycleError::InvalidSnapshot(_))
    ));

    let machine = simulated_machine()?;
    let mut snapshot = machine.snapshot();
    snapshot.transitions[2].state = TransactionState::Simulated {
        candidate_digest: "0xchanged".to_owned(),
    };
    assert_eq!(
        TransactionStateMachine::restore(snapshot, None),
        Err(TransactionLifecycleError::MismatchedCandidate)
    );

    let machine = broadcast_machine()?;
    let mut snapshot = machine.snapshot();
    snapshot.state = TransactionState::Confirmed {
        broadcast_reference: "0xchanged".to_owned(),
    };
    snapshot.revision += 1;
    snapshot.transitions.push(TransactionTransition {
        revision: snapshot.revision,
        from: Some(TransactionStateKind::Broadcast),
        state: snapshot.state.clone(),
        reason: TransactionTransitionReason::ReceiptConfirmed,
    });
    assert!(matches!(
        TransactionStateMachine::restore(snapshot, None),
        Err(TransactionLifecycleError::InvalidSnapshot(_))
    ));
    Ok(())
}

#[test]
fn snapshots_round_trip_without_transition_authority() -> Result<(), Box<dyn std::error::Error>> {
    let machine = broadcast_machine()?;
    let encoded = serde_json::to_string(&machine.snapshot())?;
    for forbidden in [
        "private_key",
        "recovery_phrase",
        "seed_phrase",
        "signing_payload",
        "signed_transaction",
        "raw_signature",
        "approval_capability",
    ] {
        assert!(!encoded.contains(forbidden));
    }
    let decoded: TransactionStateSnapshot = serde_json::from_str(&encoded)?;
    let evidence = RestartEvidence::broadcast(CANDIDATE, TRANSACTION_HASH)?;
    let restored = TransactionStateMachine::restore(decoded, Some(&evidence))?;
    assert_eq!(restored.state().kind(), TransactionStateKind::Broadcast);
    Ok(())
}
