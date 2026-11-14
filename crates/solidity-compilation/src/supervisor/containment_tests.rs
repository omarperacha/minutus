//! Actual pinned Swift lifetime-guardian probe, deliberately below compilation.
//! The fixed busy JavaScript is containment input only. No reproduced artifact,
//! compiler correspondence or production checksum bypass is created here.
use std::{
    error::Error,
    io::{Read, Write},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
use worker_transport::{Error as TransportError, PackagedWorker, Role};

#[test]
#[ignore = "external owner must observe busy Swift worker CPU then kill only this private app's compiler broker"]
fn broker_loss_while_real_swift_jsc_is_busy_has_no_reaping_receipt() -> Result<(), Box<dyn Error>> {
    let cancelled = AtomicBool::new(false);
    let worker = PackagedWorker::packaged(Role::Compiler)?;
    let mut channel = worker.start(&cancelled)?;
    let mut input = channel.take_stdin()?;
    let mut output = channel.take_stdout()?;
    let flags = rustix::fs::fcntl_getfl(&output)?;
    rustix::fs::fcntl_setfl(&output, flags | rustix::fs::OFlags::NONBLOCK)?;
    // This tiny fixed request enters only the pinned helper's authority-free
    // pipe. It is not accepted by TrustedCompiler's official-checksum API.
    input.write_all(
        br#"{"compiler":"while(true){}","version":"0.8.38+commit.1835d247","input":"{}"}"#,
    )?;
    input.flush()?;
    drop(input);
    println!("compiler-busy-input-written");
    std::io::stdout().flush()?;
    let started = Instant::now();
    let (mut ended, mut eof) = (false, false);
    while started.elapsed() < Duration::from_secs(20) {
        match channel.check() {
            Ok(()) => {}
            Err(TransportError::UnconfirmedTermination) => ended = true,
            Err(error) => return Err(error.into()),
        }
        match output.read(&mut [0]) {
            Ok(0) => eof = true,
            Ok(_) => return Err("busy JavaScript unexpectedly produced output".into()),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error.into()),
        }
        if ended && eof {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(ended, "external owner must terminate the original broker");
    assert!(
        eof,
        "the Swift worker must stop and close its actual stdout"
    );
    assert!(matches!(
        channel.cancel_and_reap(),
        Err(TransportError::UnconfirmedTermination)
    ));
    Ok(())
}
