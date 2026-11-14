use super::*;
use std::os::fd::OwnedFd;

#[test]
fn stopped_reader_does_not_wait_for_remote_eof() -> Result<(), Box<dyn std::error::Error>> {
    let (read, _held_writer) = io::pipe()?;
    let state = IoState::new(Instant::now());
    let mut input = WorkerIo::new(File::from(OwnedFd::from(read)), state.clone())?;
    std::thread::scope(|scope| {
        let reader = scope.spawn(move || input.read(&mut [0; 1]));
        std::thread::sleep(Duration::from_millis(20));
        state.stopped.store(true, Ordering::Release);
        assert_eq!(
            reader.join().ok().and_then(Result::err).map(|e| e.kind()),
            Some(io::ErrorKind::TimedOut)
        );
    });
    Ok(())
}
#[test]
fn stopped_full_writer_does_not_wait_for_remote_reads() -> Result<(), Box<dyn std::error::Error>> {
    let (_held_reader, write) = io::pipe()?;
    let state = IoState::new(Instant::now());
    let mut output = WorkerIo::new(File::from(OwnedFd::from(write)), state.clone())?;
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || output.write_all(&vec![0; 1024 * 1024]));
        std::thread::sleep(Duration::from_millis(20));
        state.stopped.store(true, Ordering::Release);
        assert_eq!(
            writer.join().ok().and_then(Result::err).map(|e| e.kind()),
            Some(io::ErrorKind::TimedOut)
        );
    });
    Ok(())
}
#[test]
fn original_deadline_and_anonymous_pipe_type_are_mandatory()
-> Result<(), Box<dyn std::error::Error>> {
    let (read, _held_writer) = io::pipe()?;
    let state = IoState::new(
        Instant::now()
            .checked_sub(resources::WALL)
            .ok_or("clock underflow")?,
    );
    let mut input = WorkerIo::new(File::from(OwnedFd::from(read)), state)?;
    assert_eq!(
        input.read(&mut [0; 1]).err().map(|e| e.kind()),
        Some(io::ErrorKind::TimedOut)
    );
    assert!(WorkerIo::new(tempfile::tempfile()?, IoState::new(Instant::now())).is_err());
    Ok(())
}
