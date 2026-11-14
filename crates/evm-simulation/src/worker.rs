//! A single worker run uses only inherited typed pipes. No endpoint, key, file
//! grant, signing handle, thread pool or send API enters this process.
use crate::{
    SimulationError,
    protocol::{self, HostMessage, StatePipe, WorkerMessage},
};
use std::io::{Read, Write};

/// Executes one bounded protocol request in a dedicated process.
///
/// # Errors
/// Fails closed on unsupported resource policy, malformed pipes or execution.
/// This entry point is for the fixed helper binary, never the desktop process.
pub fn run_worker(mut input: impl Read, mut output: impl Write) -> Result<(), SimulationError> {
    crate::resources::apply()?;
    let HostMessage::Start {
        version: 1,
        request,
    } = protocol::read_frame(&mut input)?
    else {
        return Err(SimulationError::Protocol);
    };
    let result = crate::engine::execute(&request, StatePipe::new(&mut input, &mut output));
    protocol::write_frame(&WorkerMessage::Finished { result }, &mut output)?;
    Ok(())
}
