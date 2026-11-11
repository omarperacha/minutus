//! Public source inputs are fetched at fixed origins and locally reproduced.
//! Provider verification flags/ABI/output are never accepted as authority.
use super::GenericError;
use crate::{EvmNetwork, PublicEvmAddress};
use reqwest::{blocking::Client, redirect::Policy};
use serde_json::Value;
use solidity_compilation::{CompilationInput, ReproducedArtifact, TrustedCompiler};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
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
        if directory
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(GenericError::SourceMismatch);
        }
        fs::create_dir_all(directory).map_err(|_| GenericError::SourceMismatch)?;
        Ok(Self {
            compiler: TrustedCompiler::packaged().map_err(|_| GenericError::SourceMismatch)?,
            directory: directory.into(),
            artifacts: BTreeMap::new(),
        })
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
        let target = PublicEvmAddress::new(target).map_err(|_| GenericError::SourceMismatch)?;
        let chain = network.descriptor().chain_id();
        let key = (
            chain,
            target.as_str().into(),
            format!("{:#x}", alloy_primitives::keccak256(runtime)),
        );
        if let Some(artifact) = self.artifacts.get(&key) {
            return Ok(Arc::clone(artifact));
        }
        if runtime.is_empty() || runtime.len() > 24_576 || self.artifacts.len() >= 32 {
            return Err(GenericError::Limit);
        }
        let url = format!(
            "https://sourcify.dev/server/v2/contract/{chain}/{}?fields=all",
            target.as_str()
        );
        let bytes = get(&url, 8 * 1024 * 1024, cancelled, started)?;
        let response: Value =
            serde_json::from_slice(&bytes).map_err(|_| GenericError::SourceMismatch)?;
        // Chain/address are correspondence assertions, not trusted compiler
        // results. Still reject cross-address substitutions before compilation.
        if response["chainId"].as_str() != Some(&chain.to_string())
            || response["address"]
                .as_str()
                .and_then(|v| PublicEvmAddress::new(v).ok())
                .as_ref()
                .map(PublicEvmAddress::as_str)
                != Some(target.as_str())
        {
            return Err(GenericError::SourceMismatch);
        }
        let compilation = &response["compilation"];
        let version = compilation["compilerVersion"]
            .as_str()
            .ok_or(GenericError::SourceMismatch)?;
        let qualified = compilation["fullyQualifiedName"]
            .as_str()
            .ok_or(GenericError::SourceMismatch)?;
        let (file, name) = qualified
            .rsplit_once(':')
            .ok_or(GenericError::SourceMismatch)?;
        let input = CompilationInput::new(
            version,
            file,
            name,
            response
                .get("stdJsonInput")
                .ok_or(GenericError::SourceMismatch)?
                .clone(),
        )
        .map_err(|_| GenericError::SourceMismatch)?;
        current(cancelled, started)?;
        let compiler = self.compiler_bytes(input.compiler(), cancelled, started)?;
        let artifact = self
            .compiler
            .reproduce_for_runtime(&input, &compiler, runtime, cancelled)
            .map_err(|error| {
                #[cfg(debug_assertions)]
                if std::env::var_os("MINUTUS_SOURCE_QUALIFICATION_DIAGNOSTICS")
                    .is_some_and(|value| value == "1")
                {
                    // Closed local enum only: never provider prose, source,
                    // calldata, paths or signing material.
                    eprintln!("Compiler qualification diagnostic: {error:?}");
                }
                #[cfg(not(debug_assertions))]
                let _ = error;
                GenericError::SourceMismatch
            })?;
        current(cancelled, started)?;
        if artifact.runtime() != runtime {
            return Err(GenericError::SourceMismatch);
        }
        let artifact = Arc::new(artifact);
        self.artifacts.insert(key, Arc::clone(&artifact));
        Ok(artifact)
    }
    fn compiler_bytes(
        &self,
        build: &solidity_compilation::CompilerBuild,
        cancelled: &AtomicBool,
        started: Instant,
    ) -> Result<Vec<u8>, GenericError> {
        let licence = self.directory.join("Solidity-GPL-3.0.txt");
        if let Ok(mut file) = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(licence)
        {
            file.write_all(build.licence().as_bytes())
                .map_err(|_| GenericError::SourceMismatch)?;
        }
        let provenance = self
            .directory
            .join(format!("{}.source.json", build.cache_name()));
        if let Ok(mut file) = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(provenance)
        {
            let record = serde_json::json!({"compiler":build.version(),"download":build.url(),
                "source":build.source_url(),"licence":"GPL-3.0","unmodified":true});
            file.write_all(
                &serde_json::to_vec_pretty(&record).map_err(|_| GenericError::SourceMismatch)?,
            )
            .map_err(|_| GenericError::SourceMismatch)?;
        }
        let path = self.directory.join(build.cache_name());
        if path.exists() || path.is_symlink() {
            let descriptor = rustix::fs::open(
                &path,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|_| GenericError::SourceMismatch)?;
            let file = fs::File::from(descriptor);
            if !file
                .metadata()
                .map_err(|_| GenericError::SourceMismatch)?
                .is_file()
            {
                return Err(GenericError::SourceMismatch);
            }
            let mut bytes = Vec::new();
            file.take(40 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| GenericError::SourceMismatch)?;
            if bytes.len() > 40 * 1024 * 1024 {
                return Err(GenericError::Limit);
            }
            // reproduce independently verifies the exact official hash before
            // any compiler bytes enter the helper; cache contents grant no trust.
            return Ok(bytes);
        }
        let bytes = get(&build.url(), 40 * 1024 * 1024, cancelled, started)?;
        build
            .verify(&bytes)
            .map_err(|_| GenericError::SourceMismatch)?;
        // Persist only the data, exclusively and without following symlinks.
        // Another writer winning creation is harmless: this run compiles these
        // exact bytes and verifies their checksum, future reads reverify again.
        if let Ok(mut file) = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            file.write_all(&bytes)
                .map_err(|_| GenericError::SourceMismatch)?;
            file.sync_all().map_err(|_| GenericError::SourceMismatch)?;
        }
        Ok(bytes)
    }
}
fn current(cancelled: &AtomicBool, started: Instant) -> Result<(), GenericError> {
    evm_simulation::ensure_worker_runtime_available()
        .map_err(|_| GenericError::ExecutionRejected)?;
    if cancelled.load(Ordering::Acquire) || started.elapsed() > Duration::from_secs(90) {
        return Err(GenericError::Expired);
    }
    Ok(())
}
fn get(
    url: &str,
    limit: u64,
    cancelled: &AtomicBool,
    started: Instant,
) -> Result<Vec<u8>, GenericError> {
    current(cancelled, started)?;
    let client = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .user_agent("Minutus source reproduction/1")
        .build()
        .map_err(|_| GenericError::SourceMismatch)?;
    let request = client.get(url);
    current(cancelled, started)?;
    let mut response = request.send().map_err(|_| GenericError::SourceMismatch)?;
    if !response.status().is_success() || response.content_length().is_some_and(|n| n > limit) {
        return Err(GenericError::SourceMismatch);
    }
    let mut output = Vec::new();
    response
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut output)
        .map_err(|_| GenericError::SourceMismatch)?;
    current(cancelled, started)?;
    if output.len() as u64 > limit {
        return Err(GenericError::Limit);
    }
    Ok(output)
}
