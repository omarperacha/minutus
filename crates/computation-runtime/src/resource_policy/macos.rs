//! Read-only accounting for self or the supervisor's own unreaped child.
//!
//! App Sandbox denies `proc_pid_rusage` for a child. A task-name right permits
//! memory statistics without granting task control or reading its address space.

#![allow(unsafe_code)]

use std::{mem, time::Duration};

use mach2::{
    kern_return::KERN_SUCCESS,
    mach_port::mach_port_deallocate,
    mach_time::{mach_timebase_info, mach_timebase_info_data_t},
    port::{MACH_PORT_NULL, mach_port_t},
    task::task_info,
    task_info::{TASK_VM_INFO, task_vm_info},
    traps::mach_task_self,
};

use super::ProcessUsage;

unsafe extern "C" {
    fn task_name_for_pid(task: mach_port_t, pid: libc::pid_t, name: *mut mach_port_t) -> i32;
}

const VM_REV1_COUNT: u32 = 38;
const PROC_FLAG_INEXIT: u32 = 4;

struct TaskName(mach_port_t);

impl Drop for TaskName {
    fn drop(&mut self) {
        // SAFETY: this right was acquired by task_name_for_pid and is released
        // exactly once. It is never exposed outside this accounting module.
        unsafe { mach_port_deallocate(mach_task_self(), self.0) };
    }
}

pub(super) fn process_usage(pid: u32) -> Result<ProcessUsage, ()> {
    let pid = i32::try_from(pid).map_err(|_| ())?;
    if pid <= 0 {
        return Err(());
    }
    let mut timebase = mach_timebase_info_data_t { numer: 0, denom: 0 };
    // SAFETY: writable exact timebase output, used only on success.
    if unsafe { mach_timebase_info(&raw mut timebase) } != KERN_SUCCESS {
        return Err(());
    }
    // Keep the self/unsandboxed path, which also preserves final accounting for
    // an exited but unreaped child. App Sandbox rejects this call for a live
    // child; the equally strict read-only task-name path below handles that case.
    let mut usage = mem::MaybeUninit::<libc::rusage_info_v0>::zeroed();
    // SAFETY: exact requested V0 output layout, only read after kernel success.
    if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V0, usage.as_mut_ptr().cast()) } == 0 {
        // SAFETY: the successful call initialized the complete V0 structure.
        let usage = unsafe { usage.assume_init() };
        return Ok(ProcessUsage {
            cpu_time: ticks_to_duration(
                usage.ri_user_time,
                usage.ri_system_time,
                timebase.numer,
                timebase.denom,
            )?,
            physical_bytes: usage.ri_phys_footprint.max(usage.ri_resident_size),
        });
    }
    // SAFETY: proc_taskinfo is a C output structure of scalar integer fields;
    // zero is valid for every field, and only a complete kernel write is read.
    let mut task = unsafe { mem::zeroed::<libc::proc_taskinfo>() };
    let mut name = MACH_PORT_NULL;
    // SAFETY: exact SDK-compatible output structures and their sizes are passed;
    // the kernel retains no pointers. Only our own unreaped child (or self) is
    // supplied by the owning supervisor/engine, never a request-provided PID.
    let length = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            (&raw mut task).cast(),
            i32::try_from(mem::size_of_val(&task)).map_err(|_| ())?,
        )
    };
    if usize::try_from(length).ok() != Some(mem::size_of_val(&task)) {
        return Err(());
    }
    // SAFETY: writable scalar output; only a read-only task-name right is asked
    // for, without task_for_pid, entitlements, or any task-control capability.
    let named = unsafe { task_name_for_pid(mach_task_self(), pid, &raw mut name) };
    if named != KERN_SUCCESS || name == MACH_PORT_NULL {
        return Err(());
    }
    let name = TaskName(name);
    let mut memory = task_vm_info::default();
    let mut count =
        u32::try_from(mem::size_of_val(&memory) / mem::size_of::<i32>()).map_err(|_| ())?;
    // REV1 ends immediately after phys_footprint; later SDK fields are optional.
    // SAFETY: mach2 supplies the packed SDK layout. Count describes the complete
    // writable buffer; success and the required prefix length are checked.
    let vm_result = unsafe {
        task_info(
            name.0,
            TASK_VM_INFO,
            (&raw mut memory).cast(),
            &raw mut count,
        )
    };
    if vm_result != KERN_SUCCESS || count < VM_REV1_COUNT {
        return Err(());
    }
    Ok(ProcessUsage {
        cpu_time: ticks_to_duration(
            task.pti_total_user,
            task.pti_total_system,
            timebase.numer,
            timebase.denom,
        )?,
        physical_bytes: memory.phys_footprint.max(memory.resident_size),
    })
}

fn ticks_to_duration(user: u64, system: u64, numer: u32, denom: u32) -> Result<Duration, ()> {
    if numer == 0 || denom == 0 {
        return Err(());
    }
    let ticks = user.checked_add(system).ok_or(())?;
    let nanos = u128::from(ticks) * u128::from(numer) / u128::from(denom);
    Ok(Duration::from_nanos(u64::try_from(nanos).map_err(|_| ())?))
}

pub(super) fn committed_to_exit(pid: u32) -> bool {
    let Ok(native_pid) = i32::try_from(pid) else {
        return false;
    };
    if native_pid <= 0 {
        return false;
    }
    // SAFETY: scalar-only SDK output, valid zero representation. The kernel
    // retains no pointer; full size, exact PID and its exit flag are required.
    let mut process = unsafe { mem::zeroed::<libc::proc_bsdinfo>() };
    let Ok(size) = i32::try_from(mem::size_of_val(&process)) else {
        return false;
    };
    // SAFETY: exact writable SDK buffer. arg=1 includes unreaped zombies. The
    // caller owns this unreaped PID, so it cannot be reused during the query.
    let length = unsafe {
        libc::proc_pidinfo(
            native_pid,
            libc::PROC_PIDTBSDINFO,
            1,
            (&raw mut process).cast(),
            size,
        )
    };
    length == size && process.pbi_pid == pid && process.pbi_flags & PROC_FLAG_INEXIT != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_exit_commit_requires_the_exact_unreaped_child()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(!committed_to_exit(0));
        assert!(!committed_to_exit(u32::MAX));
        assert!(!committed_to_exit(std::process::id()));
        let mut child = std::process::Command::new("/usr/bin/true").spawn()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut observed = committed_to_exit(child.id());
        while !observed && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
            observed = committed_to_exit(child.id());
        }
        child.wait()?;
        assert!(
            observed,
            "kernel exit commitment was not available before reap"
        );
        Ok(())
    }

    #[test]
    fn mach_ticks_are_converted_before_enforcing_cpu_seconds() -> Result<(), ()> {
        assert_eq!(
            ticks_to_duration(48_000_000, 24_000_000, 125, 3)?,
            Duration::from_secs(3)
        );
        assert_eq!(ticks_to_duration(1, 2, 1, 1)?, Duration::from_nanos(3));
        assert!(ticks_to_duration(1, 0, 0, 1).is_err());
        assert!(ticks_to_duration(1, 0, 1, 0).is_err());
        assert!(ticks_to_duration(u64::MAX, 1, 1, 1).is_err());
        assert!(ticks_to_duration(u64::MAX, 0, 125, 3).is_err());
        Ok(())
    }

    #[test]
    fn live_self_and_unreaped_child_accounting_are_available()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(
            process_usage(std::process::id())
                .map_err(|()| "self accounting failed")?
                .physical_bytes
                > 0
        );
        let mut child = std::process::Command::new("/bin/sleep").arg("1").spawn()?;
        let sample = process_usage(child.id());
        child.kill()?;
        child.wait()?;
        assert!(sample.is_ok(), "owned child accounting failed");
        assert!(process_usage(0).is_err());
        assert!(process_usage(u32::MAX).is_err());
        Ok(())
    }
}
