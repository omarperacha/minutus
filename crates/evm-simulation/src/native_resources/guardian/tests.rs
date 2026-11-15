use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn read_only_pipe_is_required_instead_of_files_or_the_write_end()
-> Result<(), Box<dyn std::error::Error>> {
    let (read, write) = io::pipe()?;
    validate(read.as_raw_fd())?;
    assert!(validate(write.as_raw_fd()).is_err());
    let file = File::open("/dev/null")?;
    assert!(validate(file.as_raw_fd()).is_err());
    assert!(validate(-1).is_err());
    Ok(())
}
#[test]
fn guardian_child_probe() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("MINUTUS_TEST_GUARDIAN_CHILD").is_none() {
        return Ok(());
    }
    start(take()?)?;
    let started = Instant::now();
    let mut work = 0u64;
    // The parent receives Ready only after actual computation with the guardian
    // already active, then closes the owner while computation continues.
    while started.elapsed() < Duration::from_millis(30) {
        work = std::hint::black_box(work.wrapping_add(1));
    }
    println!("guardian-ready");
    while started.elapsed() < Duration::from_secs(5) {
        work = std::hint::black_box(work.wrapping_add(1));
    }
    Err("guardian did not stop busy worker".into())
}
#[test]
fn losing_actual_owner_pipe_stops_the_owned_worker_without_a_control_message()
-> Result<(), Box<dyn std::error::Error>> {
    owner_failure(false)
}
#[test]
fn unexpected_owner_byte_also_stops_the_worker() -> Result<(), Box<dyn std::error::Error>> {
    owner_failure(true)
}
fn owner_failure(send_byte: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "native_resources::guardian::tests::guardian_child_probe",
            "--exact",
            "--nocapture",
        ])
        .env("MINUTUS_TEST_GUARDIAN_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut owner = configure(&mut command)?;
    let mut child = Owned(command.spawn()?);
    drop(command);
    let mut output = BufReader::new(child.0.stdout.take().ok_or("missing output")?);
    let mut line = String::new();
    while !line.contains("guardian-ready") {
        line.clear();
        if output.read_line(&mut line)? == 0 {
            return Err("guardian child failed startup".into());
        }
    }
    if send_byte {
        owner.write_all(&[1])?;
    }
    drop(owner);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Some(status) = child.0.try_wait()? {
            assert!(!status.success());
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("owned worker survived guardian EOF".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
