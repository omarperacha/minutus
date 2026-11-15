//! Public source inputs are fetched at fixed origins and locally reproduced.
//! Provider verification flags/ABI/output are never accepted as authority.
use super::super::GenericError;
use super::failure::{FailureClock, SourceFailureReason as Reason, SourceFailureStage as Stage};
use super::{compiler_cache, http, source_input};
use crate::{EvmNetwork, PublicEvmAddress};
use solidity_compilation::{ReproducedArtifact, TrustedCompiler};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Host-owned source/official-compiler acquisition. The path is an application
/// cache chosen by native composition, never a model argument or grant.
pub struct GenericSourceProvider {
    compiler: TrustedCompiler,
    directory: PathBuf,
    artifacts: BTreeMap<(u64, String, String), Arc<ReproducedArtifact>>,
}
impl GenericSourceProvider {
    /// Requires the packaged attested compiler and a separate public-artifact
    /// cache. Native outbound source/RPC consent must precede `artifact` calls.
    ///
    /// # Errors
    /// Rejects missing helper pins and indirect cache locations.
    pub fn packaged(directory: &Path) -> Result<Self, GenericError> {
        let clock = FailureClock::start(Instant::now(), Stage::ProviderSetup);
        if directory
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(clock.fail(Reason::CacheLocation));
        }
        fs::create_dir_all(directory).map_err(|_| clock.fail(Reason::CacheIo))?;
        let provider = Self {
            compiler: TrustedCompiler::packaged()
                .map_err(|error| clock.fail(Reason::Compiler(error)))?,
            directory: directory.into(),
            artifacts: BTreeMap::new(),
        };
        clock.success();
        Ok(provider)
    }
    /// Fetches self-contained public Solidity inputs and compiles them locally.
    /// Every returned artifact must equal the complete independently proved
    /// runtime supplied by the chain owner, including metadata. ABI/layout are
    /// exclusively compiler output. No calldata/signature/account secret leaves
    /// the device; the source service sees only chain/address lookup metadata.
    ///
    /// # Errors
    /// Rejects unavailable/malformed source, unsupported compilers, checksum,
    /// source/runtime mismatches, cancellation and fixed response/time limits.
    pub fn artifact(
        &mut self,
        network: EvmNetwork,
        target: &str,
        runtime: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<Arc<ReproducedArtifact>, GenericError> {
        let started = Instant::now();
        current(cancelled, started)?;
        let clock = FailureClock::start(started, Stage::SourceInput);
        let target =
            PublicEvmAddress::new(target).map_err(|_| clock.fail(Reason::InvalidTarget))?;
        let chain = network.descriptor().chain_id();
        let key = (
            chain,
            target.as_str().into(),
            format!("{:#x}", alloy_primitives::keccak256(runtime)),
        );
        if let Some(artifact) = self.artifacts.get(&key) {
            FailureClock::start(started, Stage::ArtifactCache).success();
            return Ok(Arc::clone(artifact));
        }
        if runtime.is_empty() || runtime.len() > 24_576 || self.artifacts.len() >= 32 {
            return Err(GenericError::Limit);
        }
        let url = format!(
            "https://sourcify.dev/server/v2/contract/{chain}/{}?fields=all",
            target.as_str()
        );
        let bytes = http::get(&url, 8 * 1024 * 1024, cancelled, started, Stage::SourceHttp)?;
        let input = source_input::parse(&bytes, chain, &target, started)?;
        current(cancelled, started)?;
        let compiler =
            compiler_cache::bytes(&self.directory, input.compiler(), cancelled, started)?;
        let clock = FailureClock::start(started, Stage::CompilerExecution);
        let artifact = self
            .compiler
            .reproduce_for_runtime(&input, &compiler, runtime, cancelled)
            .map_err(|error| clock.fail(Reason::Compiler(error)))?;
        clock.success();
        current(cancelled, started)?;
        let clock = FailureClock::start(started, Stage::RuntimeComparison);
        if artifact.runtime() != runtime {
            return Err(clock.fail(Reason::RuntimeMismatch));
        }
        clock.success();
        let artifact = Arc::new(artifact);
        self.artifacts.insert(key, Arc::clone(&artifact));
        Ok(artifact)
    }
}
pub(super) fn current(cancelled: &AtomicBool, started: Instant) -> Result<(), GenericError> {
    evm_simulation::ensure_worker_runtime_available()
        .map_err(|_| GenericError::ExecutionRejected)?;
    if cancelled.load(Ordering::Acquire) || started.elapsed() > Duration::from_secs(90) {
        return Err(GenericError::Expired);
    }
    Ok(())
}
