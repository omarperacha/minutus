//! Fixed child process limits supplement the interpreter/protocol budgets.
use crate::SimulationError;
use rustix::process::{Resource, Rlimit, setrlimit};

pub(crate) const WALL: std::time::Duration = std::time::Duration::from_secs(15);
pub(crate) const MEMORY: u64 = 256 * 1024 * 1024;
pub(crate) fn apply() -> Result<(), SimulationError> {
    crate::native_resources::close_inherited_descriptors()?;
    for (resource, soft, hard) in [
        (Resource::Core, 0, 0),
        (Resource::Cpu, 3, 4),
        (Resource::Nofile, 16, 16),
    ] {
        setrlimit(
            resource,
            Rlimit {
                current: Some(soft),
                maximum: Some(hard),
            },
        )
        .map_err(|_| SimulationError::WorkerUnavailable)?;
    }
    // macOS address-space limits do not reliably bound allocator mappings.
    // The parent independently measures physical footprint there and kills the
    // exact unreaped child. Linux additionally gets a hard address-space limit.
    #[cfg(target_os = "linux")]
    setrlimit(
        Resource::As,
        Rlimit {
            current: Some(MEMORY),
            maximum: Some(MEMORY),
        },
    )
    .map_err(|_| SimulationError::WorkerUnavailable)?;
    Ok(())
}
