use super::*;

#[test]
fn exited_child_response_can_finish_after_more_than_one_poll()
-> Result<(), Box<dyn std::error::Error>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let decoder = thread::spawn(move || {
        thread::sleep(POLL_INTERVAL * 4);
        sender.send(Ok(ComputationResponse::terminal(
            "delayed",
            RunnerStatus::CpuLimit,
            None,
        )))
    });
    let result = response_after_exit(&receiver, Instant::now() + Duration::from_secs(1), &|| {
        false
    })?;
    decoder.join().map_err(|_| "decoder panicked")??;
    assert_eq!(result.status(), RunnerStatus::CpuLimit);
    Ok(())
}

#[test]
fn exited_child_response_still_observes_cancellation_and_deadline() {
    let (_sender, receiver) = mpsc::sync_channel(1);
    assert_eq!(
        response_after_exit(&receiver, Instant::now() + WALL_TIME_LIMIT, &|| true),
        Err(RunnerError::Cancelled)
    );
    assert_eq!(
        response_after_exit(&receiver, Instant::now(), &|| false),
        Err(RunnerError::DeadlineExceeded)
    );
}
