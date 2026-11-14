//! Fixed-origin bounded retrieval; only native HTTP classifications escape.
use super::super::GenericError;
use super::{
    failure::{
        FailureClock, SourceFailureReason as Reason, SourceFailureStage as Stage, body_reason,
        http_reason,
    },
    provider::current,
};
use reqwest::{blocking::Client, redirect::Policy};
use std::{
    io::Read,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

pub(super) fn get(
    url: &str,
    limit: u64,
    cancelled: &AtomicBool,
    started: Instant,
    stage: Stage,
) -> Result<Vec<u8>, GenericError> {
    let clock = FailureClock::start(started, stage);
    current(cancelled, started)?;
    let client = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .user_agent("Minutus source reproduction/1")
        .build()
        .map_err(|error| clock.fail(http_reason(&error)))?;
    let request = client.get(url);
    current(cancelled, started)?;
    let mut response = request
        .send()
        .map_err(|error| clock.fail(http_reason(&error)))?;
    if !response.status().is_success() {
        return Err(clock.http_status(response.status()));
    }
    if response.content_length().is_some_and(|n| n > limit) {
        return Err(clock.fail(Reason::HttpContentLength));
    }
    let mut output = Vec::new();
    response
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut output)
        .map_err(|error| clock.fail(body_reason(&error)))?;
    current(cancelled, started)?;
    if output.len() as u64 > limit {
        return Err(GenericError::Limit);
    }
    clock.success();
    Ok(output)
}
