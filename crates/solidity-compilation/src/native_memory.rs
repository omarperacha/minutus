//! Exact read-only physical accounting for the supervisor's own unreaped child.
//! A task-name right does not grant task control or address-space access.
#![allow(unsafe_code)]
use crate::CompilationError;

#[cfg(target_os = "macos")]
use mach2::{
    kern_return::KERN_SUCCESS,
    mach_port::mach_port_deallocate,
    port::{MACH_PORT_NULL, mach_port_t},
    task::task_info,
    task_info::{TASK_VM_INFO, task_vm_info},
    traps::mach_task_self,
};
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn task_name_for_pid(task: mach_port_t, pid: libc::pid_t, name: *mut mach_port_t) -> i32;
}
#[cfg(target_os = "macos")]
struct TaskName(mach_port_t);
#[cfg(target_os = "macos")]
impl Drop for TaskName {
    fn drop(&mut self) {
        // SAFETY: acquired read-only task-name right is owned exactly once.
        unsafe {
            mach_port_deallocate(mach_task_self(), self.0);
        }
    }
}
pub(crate) fn physical_bytes(pid: u32) -> Result<u64, CompilationError> {
    #[cfg(target_os = "macos")]
    {
        let pid = libc::pid_t::try_from(pid)
            .ok()
            .filter(|pid| *pid > 0)
            .ok_or(CompilationError::WorkerUnavailable)?;
        let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v0>::zeroed();
        // SAFETY: exact SDK V0 output buffer for our still-owned child; successful
        // accounting also preserves final statistics for an unreaped exit.
        if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V0, usage.as_mut_ptr().cast()) }
            == 0
        {
            // SAFETY: kernel success initialised the exact requested V0 layout.
            let usage = unsafe { usage.assume_init() };
            return Ok(usage.ri_phys_footprint.max(usage.ri_resident_size));
        }
        // App Sandbox denies live-child rusage. Exact proc/task-name queries
        // provide the same physical/resident accounting, with no extra rights.
        task_memory(pid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(CompilationError::WorkerUnavailable)
    }
}
#[cfg(target_os = "macos")]
fn task_memory(pid: libc::pid_t) -> Result<u64, CompilationError> {
    if pid <= 0 {
        return Err(CompilationError::WorkerUnavailable);
    }
    // SAFETY: SDK scalar-only proc_taskinfo has a valid zero representation.
    let mut task = unsafe { std::mem::zeroed::<libc::proc_taskinfo>() };
    let size = i32::try_from(std::mem::size_of_val(&task))
        .map_err(|_| CompilationError::WorkerUnavailable)?;
    // SAFETY: exact writable SDK buffer; no process control or memory contents.
    let length =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, (&raw mut task).cast(), size) };
    if length != size {
        return Err(CompilationError::WorkerUnavailable);
    }
    let mut port = MACH_PORT_NULL;
    // SAFETY: asks only a read-only task-name right for the owned child; no task
    // control right, task_for_pid, arbitrary supplied PID or entitlement change.
    if unsafe { task_name_for_pid(mach_task_self(), pid, &raw mut port) } != KERN_SUCCESS
        || port == MACH_PORT_NULL
    {
        return Err(CompilationError::WorkerUnavailable);
    }
    let name = TaskName(port);
    let mut memory = task_vm_info::default();
    let capacity = u32::try_from(std::mem::size_of_val(&memory) / std::mem::size_of::<i32>())
        .map_err(|_| CompilationError::WorkerUnavailable)?;
    let mut count = capacity;
    // SAFETY: maintained mach2 SDK layout and full writable count. Only exact
    // kernel success plus the physical-footprint revision prefix is accepted.
    let result = unsafe {
        task_info(
            name.0,
            TASK_VM_INFO,
            (&raw mut memory).cast(),
            &raw mut count,
        )
    };
    if result != KERN_SUCCESS {
        return Err(CompilationError::WorkerUnavailable);
    }
    validate_memory(&memory, count, capacity)
}
#[cfg(target_os = "macos")]
fn validate_memory(
    memory: &task_vm_info,
    count: u32,
    capacity: u32,
) -> Result<u64, CompilationError> {
    const REV1_COUNT: u32 = 38;
    if count < REV1_COUNT || count > capacity {
        return Err(CompilationError::WorkerUnavailable);
    }
    Ok(memory.phys_footprint.max(memory.resident_size))
}
#[cfg(all(test, target_os = "macos"))]
mod tests;
