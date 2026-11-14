//! Debug fixture data only: verify the fixed official compiler before copying.
use ipc_types::IpcError;
use solidity_compilation::compiler_build;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::Path,
};

const VERSION: &str = "0.8.7+commit.e28d00a7";
const LIMIT: u64 = 40 * 1024 * 1024;

pub(super) fn copy_verified(input: &Path, cache: &Path) -> Result<(), IpcError> {
    let build = compiler_build(VERSION).map_err(|_| failure())?;
    let descriptor = rustix::fs::open(
        input,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| failure())?;
    let file = File::from(descriptor);
    let metadata = file.metadata().map_err(|_| failure())?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return Err(failure());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failure())?;
    build.verify(&bytes).map_err(|_| failure())?;
    // Only verified compiler data is copied. The normal source broker still
    // checks its checksum and reproduces each fresh proof-bound runtime.
    fs::create_dir_all(cache).map_err(|_| failure())?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(cache.join(build.cache_name()))
        .map_err(|_| failure())?;
    destination.write_all(&bytes).map_err(|_| failure())?;
    destination.sync_all().map_err(|_| failure())?;
    Ok(())
}

fn failure() -> IpcError {
    IpcError::invalid_request(
        "Compiler fixture seed must be a bounded regular file containing the exact official 0.8.7 compiler; no seed was accepted",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_or_oversized_public_data_never_creates_a_compiler_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let input = directory.path().join("input.js");
        let cache = directory.path().join("temporary-cache");
        for content in [b"".as_slice(), b"untrusted compiler data".as_slice()] {
            fs::write(&input, content)?;
            let error = copy_verified(&input, &cache)
                .err()
                .ok_or("accepted corrupt seed")?;
            assert!(!error.message.contains(input.to_string_lossy().as_ref()));
            assert!(!cache.exists());
        }
        OpenOptions::new()
            .write(true)
            .open(&input)?
            .set_len(LIMIT + 1)?;
        assert!(copy_verified(&input, &cache).is_err());
        assert!(!cache.exists());
        assert!(copy_verified(directory.path(), &cache).is_err());
        assert!(copy_verified(&directory.path().join("missing"), &cache).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_rejected_without_modifying_the_public_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let input = directory.path().join("public.js");
        let link = directory.path().join("alias.js");
        fs::write(&input, b"synthetic public input")?;
        std::os::unix::fs::symlink(&input, &link)?;
        assert!(copy_verified(&link, &directory.path().join("cache")).is_err());
        assert_eq!(fs::read(&input)?, b"synthetic public input");
        Ok(())
    }
}
