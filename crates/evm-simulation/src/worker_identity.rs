//! Expected identities come from the loaded host image, never a caller digest.
use crate::{SimulationControl, SimulationError};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Online,
    Offline,
}
/// Opaque role-bound identity of one helper pinned into the running host build.
/// No constructor accepts a path, digest, model assertion or runtime manifest.
pub struct TrustedSimulationWorker {
    path: PathBuf,
    sha256: [u8; 32],
    cdhash: [u8; 20],
    role: Role,
}
/// The two independently pinned, separate helper executables for this host build.
pub struct TrustedSimulationWorkers {
    online: TrustedSimulationWorker,
    offline: TrustedSimulationWorker,
}
impl TrustedSimulationWorkers {
    /// Reads only the immutable manifest section embedded in the loaded main
    /// executable at host build time, and derives fixed sibling helper names.
    ///
    /// # Errors
    /// Unpackaged developer hosts, missing/malformed pins, indirect or modified
    /// helpers, unsupported OS and missing mandatory isolation fail closed.
    pub fn packaged() -> Result<Self, SimulationError> {
        let manifest = crate::native_identity::host_manifest()?;
        let root = std::env::current_exe()
            .map_err(|_| SimulationError::WorkerUnavailable)?
            .parent()
            .ok_or(SimulationError::WorkerUnavailable)?
            .to_path_buf();
        let result = parse(&manifest, &root)?;
        result.online.verify_file()?;
        result.offline.verify_file()?;
        Ok(result)
    }
    /// The online authority-free execution helper.
    #[must_use]
    pub const fn online(&self) -> &TrustedSimulationWorker {
        &self.online
    }
    /// The offline signed-envelope helper, with no network broker.
    #[must_use]
    pub const fn offline(&self) -> &TrustedSimulationWorker {
        &self.offline
    }
}
impl TrustedSimulationWorker {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn require(&self, role: Role) -> Result<(), SimulationError> {
        if self.role == role {
            Ok(())
        } else {
            Err(SimulationError::WorkerUnavailable)
        }
    }
    pub(crate) fn verify_file(&self) -> Result<(), SimulationError> {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW);
        let mut file = options
            .open(&self.path)
            .map_err(|_| SimulationError::WorkerUnavailable)?;
        let before = file
            .metadata()
            .map_err(|_| SimulationError::WorkerUnavailable)?;
        if !before.is_file() || before.len() > 256 * 1024 * 1024 {
            return Err(SimulationError::WorkerUnavailable);
        }
        if digest(&mut file)? != self.sha256 {
            return Err(SimulationError::WorkerUnavailable);
        }
        let after = file
            .metadata()
            .map_err(|_| SimulationError::WorkerUnavailable)?;
        if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Err(SimulationError::WorkerUnavailable);
        }
        Ok(())
    }
    pub(crate) fn await_running(
        &self,
        pid: u32,
        control: &SimulationControl<'_>,
    ) -> Result<(), SimulationError> {
        // sandbox-exec initially has its own Apple signature. Wait for its exec
        // transition, but release no request/state/signature material beforehand.
        for _ in 0..100 {
            control.remaining()?;
            if crate::native_identity::child_exited(pid)? {
                return Err(SimulationError::WorkerUnavailable);
            }
            if crate::native_identity::child_hash(pid).is_ok_and(|hash| hash == self.cdhash) {
                return if crate::native_identity::child_exited(pid)? {
                    Err(SimulationError::WorkerUnavailable)
                } else {
                    Ok(())
                };
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(SimulationError::WorkerUnavailable)
    }
}
fn digest(file: &mut File) -> Result<[u8; 32], SimulationError> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        let length = file
            .read(&mut buffer)
            .map_err(|_| SimulationError::WorkerUnavailable)?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(hash.finalize().into())
}
fn parse(bytes: &[u8], root: &Path) -> Result<TrustedSimulationWorkers, SimulationError> {
    let text = std::str::from_utf8(bytes).map_err(|_| SimulationError::WorkerUnavailable)?;
    let mut lines = text.lines();
    if lines.next() != Some("minutus-evm-workers-v1") {
        return Err(SimulationError::WorkerUnavailable);
    }
    let online = line(lines.next(), root, "minutus-evm-simulator", Role::Online)?;
    let offline = line(
        lines.next(),
        root,
        "minutus-base-offline-simulator",
        Role::Offline,
    )?;
    if lines.next().is_some() {
        return Err(SimulationError::WorkerUnavailable);
    }
    Ok(TrustedSimulationWorkers { online, offline })
}
fn line(
    line: Option<&str>,
    root: &Path,
    name: &str,
    role: Role,
) -> Result<TrustedSimulationWorker, SimulationError> {
    let mut fields = line.ok_or(SimulationError::WorkerUnavailable)?.split(' ');
    if fields.next() != Some(name) {
        return Err(SimulationError::WorkerUnavailable);
    }
    let sha256 = decode(fields.next())?;
    let cdhash = decode(fields.next())?;
    if fields.next().is_some() {
        return Err(SimulationError::WorkerUnavailable);
    }
    Ok(TrustedSimulationWorker {
        path: root.join(name),
        sha256,
        cdhash,
        role,
    })
}
fn decode<const N: usize>(value: Option<&str>) -> Result<[u8; N], SimulationError> {
    let value = value.ok_or(SimulationError::WorkerUnavailable)?;
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SimulationError::WorkerUnavailable);
    }
    let mut result = [0; N];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| SimulationError::WorkerUnavailable)?;
    }
    Ok(result)
}
#[cfg(test)]
mod tests;
