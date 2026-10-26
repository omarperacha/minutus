//! Fixed OS and interpreter limits for one computation process.

use std::time::Duration;

use rustix::process::{Resource, Rlimit, setrlimit};

#[cfg(target_os = "macos")]
mod macos;

/// Maximum JavaScript heap managed by `QuickJS`.
pub const JAVASCRIPT_HEAP_LIMIT_BYTES: usize = 64 * 1024 * 1024;
/// Maximum JavaScript stack managed by `QuickJS`.
pub const JAVASCRIPT_STACK_LIMIT_BYTES: usize = 512 * 1024;
/// Maximum physical/process address-space allowance.
pub const PROCESS_MEMORY_LIMIT_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum accumulated user plus system CPU time.
pub const CPU_TIME_LIMIT: Duration = Duration::from_secs(3);
/// Maximum end-to-end elapsed time including startup and IPC.
pub const WALL_TIME_LIMIT: Duration = Duration::from_secs(5);

const CPU_HARD_LIMIT_SECONDS: u64 = 4;
const MAX_OPEN_FILES: u64 = 8;
#[cfg(target_os = "macos")]
const MAX_CHILD_PROCESSES: u64 = 1;

pub(crate) fn apply_process_limits() -> Result<(), ()> {
    set(Resource::Core, 0, 0)?;
    #[cfg(target_os = "macos")]
    set(Resource::Nproc, MAX_CHILD_PROCESSES, MAX_CHILD_PROCESSES)?;
    set(
        Resource::Cpu,
        CPU_TIME_LIMIT.as_secs(),
        CPU_HARD_LIMIT_SECONDS,
    )?;
    set(Resource::Nofile, MAX_OPEN_FILES, MAX_OPEN_FILES)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    const CHILD_MODE: &str = "ZETA_TEST_COMPUTATION_PROCESS_LIMIT";

    #[test]
    fn process_policy_prevents_spawn_and_fork_bomb_primitives()
    -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var_os(CHILD_MODE).is_some() {
            apply_process_limits().map_err(|()| "process policy was unavailable")?;
            assert!(
                std::process::Command::new("/usr/bin/true")
                    .status()
                    .is_err(),
                "the runner process limit allowed a child process"
            );
            return Ok(());
        }

        let status = std::process::Command::new(std::env::current_exe()?)
            .env_clear()
            .env(CHILD_MODE, "1")
            .args([
                "--exact",
                "resource_policy::tests::process_policy_prevents_spawn_and_fork_bomb_primitives",
                "--nocapture",
            ])
            .status()?;
        assert!(status.success());
        Ok(())
    }
}

fn set(resource: Resource, current: u64, maximum: u64) -> Result<(), ()> {
    setrlimit(
        resource,
        Rlimit {
            current: Some(current),
            maximum: Some(maximum),
        },
    )
    .map_err(|_| ())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProcessUsage {
    pub(crate) cpu_time: Duration,
    pub(crate) physical_bytes: u64,
}

#[cfg(target_os = "macos")]
pub(crate) fn process_usage(pid: u32) -> Result<ProcessUsage, ()> {
    macos::process_usage(pid)
}

// A kernel exit commitment is distinct from live resource accounting. Callers
// still have to reap this exact child before accepting any result.
#[cfg(target_os = "macos")]
pub(crate) fn committed_to_exit(pid: u32) -> bool {
    macos::committed_to_exit(pid)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn committed_to_exit(_pid: u32) -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn process_usage(_pid: u32) -> Result<ProcessUsage, ()> {
    Ok(ProcessUsage::default())
}
