//! Read-only memory accounting for the supervisor's locked, unreaped child.
//! App Sandbox can deny rusage while permitting the task-name statistics path.
use crate::SimulationError;
use mach2::{
    kern_return::KERN_SUCCESS,
    mach_port::mach_port_deallocate,
    port::{MACH_PORT_NULL, mach_port_t},
    task::task_info,
    task_info::{TASK_VM_INFO, task_vm_info},
    traps::mach_task_self,
};
use std::mem;

unsafe extern "C" {
    fn task_name_for_pid(task: mach_port_t, pid: libc::pid_t, name: *mut mach_port_t) -> i32;
}
// The SDK's REV1 prefix includes phys_footprint. Newer tail fields are optional.
const VM_REV1_COUNT: u32 = 38;
struct TaskName(mach_port_t);
impl Drop for TaskName {
    fn drop(&mut self) {
        // SAFETY: this right was acquired by task_name_for_pid and is released
        // exactly once. It never leaves this private accounting module.
        unsafe { mach_port_deallocate(mach_task_self(), self.0) };
    }
}

pub(crate) fn physical_bytes(pid: u32) -> Result<u64, SimulationError> {
    let pid = i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or(SimulationError::WorkerUnavailable)?;
    let mut usage = mem::MaybeUninit::<libc::rusage_info_v0>::zeroed();
    // SAFETY: exact SDK V0 buffer. The supervisor holds this unreaped child's
    // mutex across every query, preventing reap/PID reuse during accounting.
    if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V0, usage.as_mut_ptr().cast()) } == 0 {
        // SAFETY: a successful kernel call initialised the exact V0 structure.
        let usage = unsafe { usage.assume_init() };
        return Ok(usage.ri_phys_footprint.max(usage.ri_resident_size));
    }
    task_physical_bytes(pid)
}
fn task_physical_bytes(pid: i32) -> Result<u64, SimulationError> {
    if pid <= 0 {
        return Err(SimulationError::WorkerUnavailable);
    }
    // SAFETY: this SDK structure consists of scalar integer fields; all zeroes
    // are valid. Only a complete kernel write is accepted below.
    let mut process = unsafe { mem::zeroed::<libc::proc_taskinfo>() };
    // SAFETY: exact writable SDK buffer for our locked unreaped child; no
    // pointer is retained by the kernel and no request-supplied PID is accepted.
    let length = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            (&raw mut process).cast(),
            i32::try_from(mem::size_of_val(&process))
                .map_err(|_| SimulationError::WorkerUnavailable)?,
        )
    };
    if usize::try_from(length).ok() != Some(mem::size_of_val(&process)) {
        return Err(SimulationError::WorkerUnavailable);
    }
    let mut name = MACH_PORT_NULL;
    // SAFETY: scalar output requests only a read-only task-name right. This
    // does not request task_for_pid, task control or access to guest memory.
    let named = unsafe { task_name_for_pid(mach_task_self(), pid, &raw mut name) };
    if named != KERN_SUCCESS || name == MACH_PORT_NULL {
        return Err(SimulationError::WorkerUnavailable);
    }
    let name = TaskName(name);
    let mut memory = task_vm_info::default();
    let mut count = vm_capacity()?;
    // SAFETY: mach2 provides the packed SDK layout. Count gives its full
    // writable capacity; result and the complete required prefix are checked.
    let result = unsafe {
        task_info(
            name.0,
            TASK_VM_INFO,
            (&raw mut memory).cast(),
            &raw mut count,
        )
    };
    Ok(vm_footprint(&memory, result, count)?.max(process.pti_resident_size))
}
fn vm_capacity() -> Result<u32, SimulationError> {
    u32::try_from(mem::size_of::<task_vm_info>() / mem::size_of::<i32>())
        .map_err(|_| SimulationError::WorkerUnavailable)
}
fn vm_footprint(memory: &task_vm_info, result: i32, count: u32) -> Result<u64, SimulationError> {
    if result != KERN_SUCCESS || count < VM_REV1_COUNT || count > vm_capacity()? {
        return Err(SimulationError::WorkerUnavailable);
    }
    Ok(memory.phys_footprint.max(memory.resident_size))
}

#[cfg(test)]
mod tests;
