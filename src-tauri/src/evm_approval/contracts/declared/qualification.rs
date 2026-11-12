//! Exact native preparation in an ephemeral journal, without wallet authority.
mod compiler_seed;
mod fixture;
mod projection;
mod refresh;

use super::context::Context;
use crate::evm_approval::ApprovalOrigin;
use chain_evm::EvmNetwork;
use ipc_types::IpcError;
use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

pub(crate) fn run(arguments: &[OsString]) -> Result<(), IpcError> {
    let options = Options::parse(arguments)?;
    let count = options.attempts;
    let directory = tempfile::tempdir().map_err(|_| IpcError::internal())?;
    if let Some(input) = &options.cached_compiler {
        compiler_seed::copy_verified(input, &directory.path().join("public-contract-sources"))?;
        eprintln!(
            "Compiler fixture seed: checksum-verified public compiler bytes; temporary cache only"
        );
    }
    let endpoint = Arc::new(crate::evm_rpc_settings::resolved(
        EvmNetwork::EthereumSepolia,
    )?);
    eprintln!(
        "Contract preparation RPC origin: {}",
        endpoint.public_origin()
    );
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
    cached_compiler: Option<PathBuf>,
}
impl Options {
    fn parse(arguments: &[OsString]) -> Result<Self, IpcError> {
        let mut options = Self {
            attempts: 1,
            refresh: false,
            cached_compiler: None,
        };
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            if argument == OsStr::new("--repeat") && options.attempts == 1 {
                options.attempts = 3;
            } else if argument == OsStr::new("--refresh") && !options.refresh {
                options.refresh = true;
            } else if argument == OsStr::new("--cached-compiler")
                && options.cached_compiler.is_none()
            {
                let path = arguments
                    .next()
                    .filter(|value| !value.is_empty() && !value.to_string_lossy().starts_with("--"))
                    .ok_or_else(|| {
                        IpcError::invalid_request(
                            "Expected one public compiler data path after --cached-compiler",
                        )
                    })?;
                options.cached_compiler = Some(PathBuf::from(path));
            } else {
                return Err(IpcError::invalid_request(
                    "Expected optional --refresh, --repeat and --cached-compiler path once each for the fixed public preparation fixture",
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
        assert_eq!(
            Options::parse(&[
                "--cached-compiler".into(),
                "public.js".into(),
                "--repeat".into()
            ])?
            .cached_compiler,
            Some(PathBuf::from("public.js"))
        );
        for invalid in [
            vec!["--cached-compiler"],
            vec!["--cached-compiler", ""],
            vec!["--cached-compiler", "--repeat"],
            vec!["--cached-compiler", "a.js", "--cached-compiler", "b.js"],
        ] {
            assert!(
                Options::parse(&invalid.into_iter().map(OsString::from).collect::<Vec<_>>())
                    .is_err()
            );
        }
        Ok(())
    }
}
