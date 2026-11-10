use super::*;
use serde_json::Value;
use std::io;

fn worker(path: &std::path::Path) -> Result<TrustedCompiler, Box<dyn std::error::Error>> {
    Ok(TrustedCompiler {
        path: path.into(),
        digest: format!("{:x}", Sha256::digest(fs::read(path)?)),
        cdhash: [0; 20],
    })
}
#[test]
fn file_identity_rejects_tampering_directories_and_symlinks()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("helper");
    fs::write(&path, b"pinned bytes")?;
    let pinned = worker(&path)?;
    pinned.verify_file()?;
    fs::write(&path, b"changed bytes")?;
    assert_eq!(
        pinned.verify_file(),
        Err(CompilationError::WorkerUnavailable)
    );
    fs::remove_file(&path)?;
    fs::create_dir(&path)?;
    assert_eq!(
        pinned.verify_file(),
        Err(CompilationError::WorkerUnavailable)
    );
    fs::remove_dir(&path)?;
    let target = directory.path().join("other");
    fs::write(&target, b"pinned bytes")?;
    std::os::unix::fs::symlink(&target, &path)?;
    assert_eq!(
        pinned.verify_file(),
        Err(CompilationError::WorkerUnavailable)
    );
    Ok(())
}
#[test]
fn file_identity_rejects_missing_and_oversized_executables()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("helper");
    fs::write(&path, b"pinned")?;
    let pinned = worker(&path)?;
    fs::OpenOptions::new()
        .write(true)
        .open(&path)?
        .set_len(16 * 1024 * 1024 + 1)?;
    assert_eq!(
        pinned.verify_file(),
        Err(CompilationError::WorkerUnavailable)
    );
    fs::remove_file(path)?;
    assert_eq!(
        pinned.verify_file(),
        Err(CompilationError::WorkerUnavailable)
    );
    Ok(())
}
#[test]
fn cancellation_and_expired_deadlines_reject_before_launch()
-> Result<(), Box<dyn std::error::Error>> {
    let pinned = worker(std::path::Path::new("/bin/cat"))?;
    assert!(matches!(
        pinned.launch(Instant::now(), &AtomicBool::new(true)),
        Err(CompilationError::Cancelled)
    ));
    assert!(matches!(
        pinned.launch(
            Instant::now().checked_sub(WALL).ok_or("clock underflow")?,
            &AtomicBool::new(false)
        ),
        Err(CompilationError::Limit)
    ));
    Ok(())
}
#[test]
fn bounded_output_accepts_exact_limit_and_rejects_more_or_io_errors() -> Result<(), CompilationError>
{
    const LIMIT: u64 = 32 * 1024 * 1024;
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("untrusted reader text"))
        }
    }
    assert_eq!(
        read_output(io::repeat(7).take(LIMIT))?.len(),
        usize::try_from(LIMIT).map_err(|_| CompilationError::Limit)?
    );
    assert_eq!(
        read_output(io::repeat(7).take(LIMIT + 1)),
        Err(CompilationError::Limit)
    );
    assert_eq!(
        read_output(Broken),
        Err(CompilationError::CompilationFailed)
    );
    Ok(())
}
#[test]
fn response_schema_rejects_unknown_fields_and_malformed_json() {
    for bytes in [
        b"not JSON".as_slice(),
        br#"{"version":"x","output":"{}","authority":true}"#,
        br#"{"version":7,"output":"{}"}"#,
        br#"{"version":"x"}"#,
    ] {
        assert!(serde_json::from_slice::<Response>(bytes).is_err());
    }
}

fn compiler_fixture() -> Result<(CompilationInput, Value), Box<dyn std::error::Error>> {
    Ok((
        CompilationInput::new(
            "0.8.38",
            "Counter.sol",
            "Counter",
            serde_json::from_str(include_str!("../../tests/fixtures/counter-input.json"))?,
        )?,
        serde_json::from_str(include_str!("../../tests/fixtures/counter-output.json"))?,
    ))
}
#[test]
fn response_version_output_and_artifact_must_all_match() -> Result<(), Box<dyn std::error::Error>> {
    let (input, output) = compiler_fixture()?;
    let expected = format!("{}.Emscripten.clang", input.build.version());
    let response = serde_json::json!({"version":expected,"output":serde_json::to_string(&output)?});
    decode_response(&input, &serde_json::to_vec(&response)?)?;
    for version in [
        "0.8.38",
        input.build.version(),
        "0.8.38+commit.deadbeef.Emscripten.clang",
    ] {
        let mut changed = response.clone();
        changed["version"] = serde_json::json!(version);
        assert!(matches!(
            decode_response(&input, &serde_json::to_vec(&changed)?),
            Err(CompilationError::CompilerMismatch)
        ));
    }
    for invalid in ["not JSON", "null", "{}"] {
        let mut changed = response.clone();
        changed["output"] = serde_json::json!(invalid);
        assert!(matches!(
            decode_response(&input, &serde_json::to_vec(&changed)?),
            Err(CompilationError::CompilationFailed)
        ));
    }
    Ok(())
}
#[test]
fn unofficial_compiler_bytes_reject_before_any_helper_launch()
-> Result<(), Box<dyn std::error::Error>> {
    let (input, _) = compiler_fixture()?;
    let absent = TrustedCompiler {
        path: "/missing/compiler-test-never-executed".into(),
        digest: String::new(),
        cdhash: [0; 20],
    };
    assert!(matches!(
        absent.reproduce(&input, b"changed compiler", &AtomicBool::new(false)),
        Err(CompilationError::CompilerMismatch)
    ));
    assert!(matches!(
        absent.reproduce(&input, &[], &AtomicBool::new(false)),
        Err(CompilationError::Limit)
    ));
    assert_eq!(
        input.build.verify(&vec![0; 40 * 1024 * 1024 + 1]),
        Err(CompilationError::Limit)
    );
    Ok(())
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;

    fn signed_worker(path: &str) -> Result<TrustedCompiler, Box<dyn std::error::Error>> {
        let mut pinned = worker(std::path::Path::new(path))?;
        let output = Command::new("/usr/bin/codesign")
            .args(["-d", "--verbose=4", path])
            .output()?;
        if !output.status.success() {
            return Err("fixed test executable has no valid signature".into());
        }
        let details = String::from_utf8(output.stderr)?;
        let hash = details
            .lines()
            .find_map(|line| line.strip_prefix("CDHash="))
            .ok_or("missing CDHash")?;
        if hash.len() != 40 {
            return Err("unexpected CDHash length".into());
        }
        for (place, pair) in pinned
            .cdhash
            .iter_mut()
            .zip(hash.as_bytes().chunks_exact(2))
        {
            *place = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
        }
        Ok(pinned)
    }
    #[test]
    fn genuine_sandboxed_child_is_attested_before_input_and_reaped_after_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let pinned = signed_worker("/bin/cat")?;
        let cancelled = AtomicBool::new(false);
        let started = Instant::now();
        let mut child = pinned.launch(started, &cancelled)?;
        assert_eq!(native_identity::child_hash(child.0.id())?, pinned.cdhash);
        assert!(native_identity::physical_bytes(child.0.id())? > 0);
        let mut stdin = child.0.stdin.take().ok_or("missing stdin")?;
        stdin.write_all(b"public synthetic test only")?;
        drop(stdin);
        let out = read_output(child.0.stdout.take().ok_or("missing stdout")?)?;
        assert_eq!(out, b"public synthetic test only");
        assert!(await_exit(&mut child.0, started, &cancelled)?.success());
        Ok(())
    }
    #[test]
    fn running_image_mismatch_and_early_exit_never_release_an_attested_child()
    -> Result<(), Box<dyn std::error::Error>> {
        for path in ["/bin/cat", "/usr/bin/true"] {
            let pinned = worker(std::path::Path::new(path))?;
            assert!(matches!(
                pinned.launch(Instant::now(), &AtomicBool::new(false)),
                Err(CompilationError::WorkerUnavailable)
            ));
        }
        Ok(())
    }
    #[test]
    fn nonreaping_exit_probe_keeps_pid_owned_until_wait() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut child = Reaped(Command::new("/usr/bin/true").spawn()?);
        let until = Instant::now() + Duration::from_secs(5);
        while !native_identity::child_exited(child.0.id())? {
            if Instant::now() >= until {
                return Err("fixed true helper did not exit".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(native_identity::child_exited(child.0.id())?);
        assert!(child.0.wait()?.success());
        Ok(())
    }
    #[test]
    fn timeout_and_cancelled_waits_do_not_detach_the_owned_child()
    -> Result<(), Box<dyn std::error::Error>> {
        let pinned = signed_worker("/bin/cat")?;
        for cancelled in [false, true] {
            let mut child = pinned.launch(Instant::now(), &AtomicBool::new(false))?;
            let result = await_exit(
                &mut child.0,
                Instant::now().checked_sub(WALL).ok_or("clock underflow")?,
                &AtomicBool::new(cancelled),
            );
            assert_eq!(
                result.map(|_| ()),
                Err(if cancelled {
                    CompilationError::Cancelled
                } else {
                    CompilationError::Limit
                })
            );
            // Reaped::drop owns kill + wait even on either terminal error.
        }
        Ok(())
    }
    #[test]
    fn genuine_sandboxed_stream_cannot_exceed_output_cap() -> Result<(), Box<dyn std::error::Error>>
    {
        let pinned = signed_worker("/usr/bin/yes")?;
        let mut child = pinned.launch(Instant::now(), &AtomicBool::new(false))?;
        assert_eq!(
            read_output(child.0.stdout.take().ok_or("missing stdout")?),
            Err(CompilationError::Limit)
        );
        // The bounded read stops; the RAII owner kills/reaps the writer.
        Ok(())
    }
}
