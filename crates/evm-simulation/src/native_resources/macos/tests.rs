use super::*;

#[test]
fn partial_failed_or_impossible_vm_statistics_fail_closed() -> Result<(), SimulationError> {
    let memory = task_vm_info {
        phys_footprint: 128,
        resident_size: 256,
        ..Default::default()
    };
    assert_eq!(vm_footprint(&memory, KERN_SUCCESS, VM_REV1_COUNT)?, 256);
    for (result, count) in [
        (1, VM_REV1_COUNT),
        (KERN_SUCCESS, VM_REV1_COUNT - 1),
        (KERN_SUCCESS, vm_capacity()? + 1),
    ] {
        assert!(vm_footprint(&memory, result, count).is_err());
    }
    Ok(())
}

#[test]
fn sdk_task_name_fallback_accounts_for_self_and_owned_child()
-> Result<(), Box<dyn std::error::Error>> {
    let own = i32::try_from(std::process::id())?;
    assert!(task_physical_bytes(own)? > 0);
    let mut child = std::process::Command::new("/bin/sleep").arg("1").spawn()?;
    let sample = task_physical_bytes(i32::try_from(child.id())?);
    let _ = child.kill();
    child.wait()?;
    assert!(sample? > 0);
    assert!(physical_bytes(0).is_err());
    assert!(physical_bytes(u32::MAX).is_err());
    assert!(task_physical_bytes(-1).is_err());
    Ok(())
}
