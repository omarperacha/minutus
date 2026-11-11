//! Exact native preparation in an ephemeral journal, without wallet authority.
mod fixture;
mod projection;
mod refresh;

use super::context::Context;
use crate::evm_approval::ApprovalOrigin;
use chain_evm::EvmNetwork;
use ipc_types::IpcError;
use std::{
    ffi::{OsStr, OsString},
    sync::{Arc, atomic::AtomicBool},
};

pub(crate) fn run(arguments: &[OsString]) -> Result<(), IpcError> {
    let options = Options::parse(arguments)?;
    let count = options.attempts;
    let endpoint = Arc::new(crate::evm_rpc_settings::resolved(
        EvmNetwork::EthereumSepolia,
    )?);
    eprintln!(
        "Contract preparation RPC origin: {}",
        endpoint.public_origin()
    );
    let directory = tempfile::tempdir().map_err(|_| IpcError::internal())?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut failed = false;
    for attempt in 1..=count {
        eprintln!("Contract preparation attempt: {attempt}/{count}");
        let result = fixture::with(
            &directory.path().join(format!("attempt-{attempt}")),
            |fixture| {
                tauri::async_runtime::block_on(async {
                    let context = Context::preflight(
                        fixture.session.declaration(),
                        Arc::clone(&endpoint),
                        Arc::clone(&cancelled),
                        directory.path().join("public-contract-sources"),
                        ApprovalOrigin::Chat {
                            conversation: [0; 32],
                            call: [0; 32],
                        },
                    )
                    .await?;
                    let work = fixture
                        .session
                        .claim_next()
                        .map_err(|_| {
                            IpcError::invalid_request("Ephemeral workflow step admission failed")
                        })?
                        .ok_or_else(|| {
                            IpcError::invalid_request("Ephemeral workflow step unavailable")
                        })?;
                    let (candidate, proof) = context.prepare(&work, &[]).await?;
                    eprintln!("Contract preparation stage: CoreCandidateBinding");
                    let prepared = work.bind_generic_candidate(candidate, proof).map_err(|_| {
                        IpcError::invalid_request("Ephemeral candidate binding failed")
                    })?;
                    context.remember(prepared.shared_candidate())?;
                    let refreshes = if options.refresh {
                        refresh::twice(&context, &prepared).await?
                    } else {
                        vec![]
                    };
                    projection::output(&endpoint, &prepared, attempt, &refreshes)
                })
            },
        );
        match result {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!(
                    "Contract preparation attempt {attempt} failed: {}",
                    error.message
                );
                failed = true;
            }
        }
    }
    if failed {
        return Err(IpcError::invalid_request(
            "One or more independent preparation attempts failed",
        ));
    }
    Ok(())
}

struct Options {
    attempts: u8,
    refresh: bool,
}
impl Options {
    fn parse(arguments: &[OsString]) -> Result<Self, IpcError> {
        let mut options = Self {
            attempts: 1,
            refresh: false,
        };
        for argument in arguments {
            if argument == OsStr::new("--repeat") && options.attempts == 1 {
                options.attempts = 3;
            } else if argument == OsStr::new("--refresh") && !options.refresh {
                options.refresh = true;
            } else {
                return Err(IpcError::invalid_request(
                    "Expected optional --refresh and --repeat once each for the fixed public preparation fixture",
                ));
            }
        }
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repetition_is_explicit_and_no_caller_endpoint_or_payload_is_accepted() -> Result<(), IpcError>
    {
        assert_eq!(Options::parse(&[])?.attempts, 1);
        assert_eq!(Options::parse(&["--repeat".into()])?.attempts, 3);
        assert!(Options::parse(&["--refresh".into(), "--repeat".into()])?.refresh);
        for argument in ["https://secret.invalid/key", "0x1234", "--repeat=99"] {
            assert!(Options::parse(&[argument.into()]).is_err());
        }
        assert!(Options::parse(&["--repeat".into(), "--repeat".into()]).is_err());
        Ok(())
    }
}
