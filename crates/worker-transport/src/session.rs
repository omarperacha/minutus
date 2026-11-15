const HEX: &[u8; 16] = b"0123456789abcdef";
use crate::{Error, Role, identity::Identity, native::Client};
use std::{
    fs::File,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Actual worker reaping confirmed by the authenticated original session.
/// This is cleanup evidence, never a signing/approval or effect capability.
pub struct ReapedWorker {
    exit: i32,
    resource: i32,
}
impl ReapedWorker {
    /// Exact native exit status from the broker's owned child wait.
    #[must_use]
    pub const fn exit_status(&self) -> i32 {
        self.exit
    }
    /// Fixed native resource-policy result; no child prose or data.
    #[must_use]
    pub const fn resource_status(&self) -> i32 {
        self.resource
    }
    /// Both actual child success and mandatory resource policy held.
    #[must_use]
    pub const fn success(&self) -> bool {
        self.exit == 0 && self.resource == 0
    }
}
/// Authenticated fixed-role anonymous data pipes plus original cleanup owner.
/// No raw path/PID/descriptor/peer constructor or deserialisation exists.
pub struct ReadyChannel {
    client: Client,
    input: Option<File>,
    output: Option<File>,
    started: Instant,
    budget: Duration,
    reaped: Option<(i32, i32)>,
    cleanup_started: Option<Instant>,
}
pub(crate) fn start(
    identity: &Identity,
    role: Role,
    budget: Duration,
    cancelled: &AtomicBool,
) -> Result<ReadyChannel, Error> {
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    if budget.is_zero() {
        return Err(Error::Limit);
    }
    let started = Instant::now();
    let mut random = [0; 32];
    getrandom::fill(&mut random).map_err(|_| Error::Unavailable)?;
    let nonce = random
        .iter()
        .flat_map(|v| {
            [
                char::from(HEX[usize::from(v >> 4)]),
                char::from(HEX[usize::from(v & 15)]),
            ]
        })
        .collect::<String>();
    let client = Client::open(identity, role.name(), &nonce)?;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if started.elapsed() >= budget {
            return Err(Error::Limit);
        }
        if let Some((input, output)) = client.ready()? {
            return Ok(ReadyChannel {
                client,
                input: Some(input),
                output: Some(output),
                started,
                budget,
                reaped: None,
                cleanup_started: None,
            });
        }
    }
}
impl ReadyChannel {
    /// Transfers the sole authenticated worker stdin endpoint.
    /// # Errors
    /// Rejects a second take; the channel retains cleanup ownership on error.
    pub fn take_stdin(&mut self) -> Result<File, Error> {
        self.input.take().ok_or(Error::Protocol)
    }
    /// Transfers the sole authenticated worker stdout endpoint.
    /// # Errors
    /// Rejects a second take; the channel retains cleanup ownership on error.
    pub fn take_stdout(&mut self) -> Result<File, Error> {
        self.output.take().ok_or(Error::Protocol)
    }
    /// Checks original deadline and authenticated control connection.
    /// # Errors
    /// Missing control evidence is unconfirmed termination, never successful cleanup.
    pub fn check(&self) -> Result<(), Error> {
        if self.reaped.is_some() {
            return self.client.check();
        }
        if self.started.elapsed() >= self.budget {
            return Err(Error::Limit);
        }
        self.client.check()
    }
    /// Waits within the original absolute run budget for actual child reaping.
    /// # Errors
    /// Never renews the deadline or manufactures cleanup after connection loss.
    pub fn finish(&mut self) -> Result<ReapedWorker, Error> {
        self.finish_with_deadline(
            self.budget.saturating_sub(self.started.elapsed()),
            &AtomicBool::new(false),
        )
    }
    /// Preserves caller cancellation and the smaller remaining total budget.
    /// # Errors
    /// Missing actual reaping returns `UnconfirmedTermination` after bounded cleanup.
    pub fn finish_with_deadline(
        &mut self,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<ReapedWorker, Error> {
        if let Some((exit, resource)) = self.reaped {
            self.client.check()?;
            return Ok(ReapedWorker { exit, resource });
        }
        let remaining = timeout.min(self.budget.saturating_sub(self.started.elapsed()));
        let began = Instant::now();
        loop {
            if cancelled.load(Ordering::Acquire) || began.elapsed() >= remaining {
                return match self.cancel_and_reap() {
                    Ok(_) => Err(if cancelled.load(Ordering::Acquire) {
                        Error::Cancelled
                    } else {
                        Error::Limit
                    }),
                    Err(_) => Err(Error::UnconfirmedTermination),
                };
            }
            if let Some((exit, resource)) = self.client.finish(false)? {
                self.reaped = Some((exit, resource));
                return Ok(ReapedWorker { exit, resource });
            }
        }
    }
    /// Requests cancellation and waits at most two seconds for an authenticated
    /// actual reaping receipt. The expired application budget is never renewed.
    /// # Errors
    /// `UnconfirmedTermination` must poison any signed lease/runtime owner.
    pub fn cancel_and_reap(&mut self) -> Result<ReapedWorker, Error> {
        if let Some((exit, resource)) = self.reaped {
            self.client.check()?;
            return Ok(ReapedWorker { exit, resource });
        }
        self.input.take();
        self.output.take();
        let started = *self.cleanup_started.get_or_insert_with(Instant::now);
        loop {
            if started.elapsed() >= Duration::from_secs(2) {
                return Err(Error::UnconfirmedTermination);
            }
            if let Some((exit, resource)) = self.client.finish(true)? {
                self.reaped = Some((exit, resource));
                return Ok(ReapedWorker { exit, resource });
            }
        }
    }
}
impl Drop for ReadyChannel {
    fn drop(&mut self) {
        if self.reaped.is_none() {
            let _ = self.cancel_and_reap();
        }
    }
}
