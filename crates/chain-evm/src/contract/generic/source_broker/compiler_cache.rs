//! Official compiler data cache; every byte remains independently checksum checked.
use super::super::GenericError;
use super::{
    failure::{FailureClock, SourceFailureReason as Reason, SourceFailureStage as Stage},
    http,
};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::AtomicBool,
    time::Instant,
};
pub(super) fn bytes(
    directory: &Path,
    build: &solidity_compilation::CompilerBuild,
    cancelled: &AtomicBool,
    started: Instant,
) -> Result<Vec<u8>, GenericError> {
    let clock = FailureClock::start(started, Stage::CompilerCache);
    let licence = directory.join("Solidity-GPL-3.0.txt");
    if let Ok(mut file) = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(licence)
    {
        file.write_all(build.licence().as_bytes())
            .map_err(|_| clock.fail(Reason::CacheIo))?;
    }
    let provenance = directory.join(format!("{}.source.json", build.cache_name()));
    if let Ok(mut file) = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(provenance)
    {
        let record = serde_json::json!({"compiler":build.version(),"download":build.url(),
                "source":build.source_url(),"licence":"GPL-3.0","unmodified":true});
        file.write_all(
            &serde_json::to_vec_pretty(&record).map_err(|_| clock.fail(Reason::CacheIo))?,
        )
        .map_err(|_| clock.fail(Reason::CacheIo))?;
    }
    let path = directory.join(build.cache_name());
    if path.exists() || path.is_symlink() {
        let descriptor = rustix::fs::open(
            &path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| clock.fail(Reason::CacheIo))?;
        let file = fs::File::from(descriptor);
        if !file
            .metadata()
            .map_err(|_| clock.fail(Reason::CacheIo))?
            .is_file()
        {
            return Err(clock.fail(Reason::CacheLocation));
        }
        let mut bytes = Vec::new();
        file.take(40 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| clock.fail(Reason::CacheIo))?;
        if bytes.len() > 40 * 1024 * 1024 {
            return Err(GenericError::Limit);
        }
        // reproduce independently verifies the exact official hash before
        // any compiler bytes enter the helper; cache contents grant no trust.
        clock.success();
        return Ok(bytes);
    }
    clock.success();
    let bytes = http::get(
        &build.url(),
        40 * 1024 * 1024,
        cancelled,
        started,
        Stage::CompilerHttp,
    )?;
    let clock = FailureClock::start(started, Stage::CompilerCache);
    build
        .verify(&bytes)
        .map_err(|error| clock.fail(Reason::Compiler(error)))?;
    // Persist only the data, exclusively and without following symlinks.
    // Another writer winning creation is harmless: this run compiles these
    // exact bytes and verifies their checksum, future reads reverify again.
    if let Ok(mut file) = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        file.write_all(&bytes)
            .map_err(|_| clock.fail(Reason::CacheIo))?;
        file.sync_all().map_err(|_| clock.fail(Reason::CacheIo))?;
    }
    clock.success();
    Ok(bytes)
}
