use super::*;
use std::process::{Child, Command, Stdio};
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn real_owned_child_has_strict_task_name_physical_accounting()
-> Result<(), Box<dyn std::error::Error>> {
    let child = OwnedChild(
        Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    assert!(physical_bytes(child.0.id())? > 0);
    assert!(task_memory(i32::try_from(child.0.id())?)? > 0);
    assert_eq!(physical_bytes(0), Err(CompilationError::WorkerUnavailable));
    assert_eq!(
        physical_bytes(u32::MAX),
        Err(CompilationError::WorkerUnavailable)
    );
    assert_eq!(task_memory(-1), Err(CompilationError::WorkerUnavailable));
    Ok(())
}
#[test]
fn truncated_or_excessive_kernel_count_rejects_and_both_memory_limits_are_preserved() {
    let memory = task_vm_info {
        phys_footprint: 900,
        resident_size: 300,
        ..Default::default()
    };
    assert_eq!(
        validate_memory(&memory, 37, 100),
        Err(CompilationError::WorkerUnavailable)
    );
    let memory = task_vm_info {
        phys_footprint: 900,
        resident_size: 300,
        ..Default::default()
    };
    assert_eq!(
        validate_memory(&memory, 101, 100),
        Err(CompilationError::WorkerUnavailable)
    );
    let memory = task_vm_info {
        phys_footprint: 900,
        resident_size: 300,
        ..Default::default()
    };
    assert_eq!(validate_memory(&memory, 38, 100), Ok(900));
    let memory = task_vm_info {
        phys_footprint: 300,
        resident_size: 900,
        ..Default::default()
    };
    assert_eq!(validate_memory(&memory, 38, 100), Ok(900));
}
