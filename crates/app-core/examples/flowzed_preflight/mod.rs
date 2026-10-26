//! Unpaid inspection only; no wallet, approval, signing or paid-retry imports.

mod inspect;
mod transport;

use std::time::{SystemTime, UNIX_EPOCH};

use inspect::{Expectation, inspect};
use transport::{ResourceUrl, fetch};

// Static errors only: never echo URLs, raw challenges or transport diagnostics.
pub(super) fn run() -> Result<(), &'static str> {
    let mut args = std::env::args().skip(1);
    let expectation = args.next().ok_or(USAGE)?;
    let resource = args.next().ok_or(USAGE)?;
    if args.next().is_some() {
        return Err(USAGE);
    }
    let expectation = Expectation::parse(&expectation)?;
    let resource = ResourceUrl::parse(&resource)?;
    let observation = fetch(&resource)?;
    let received_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "host clock unavailable")?
        .as_secs();
    let report = inspect(&resource, &observation, expectation, received_at)?;
    println!("{report}");
    println!("Unpaid probe only: no wallet access, authorization, signature or payment retry.");
    Ok(())
}

const USAGE: &str =
    "usage: flowzed_x402_preflight <valid|malformed|short-lived|redirect> <resourceUrl>";

#[cfg(test)]
mod tests;
