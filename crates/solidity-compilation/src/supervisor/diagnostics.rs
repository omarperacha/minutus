//! Opt-in fixed developer stages; no child text or process/request data escapes.
#[cfg(all(test, debug_assertions))]
use std::io::Read;

pub(super) fn enabled() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("MINUTUS_SOURCE_QUALIFICATION_DIAGNOSTICS").as_deref()
            == Some(std::ffi::OsStr::new("1"))
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}
pub(super) fn stage(stage: &'static str) {
    if enabled() {
        eprintln!("minutus source compiler stage: {stage}");
    }
}
/// Called only after a non-reaping kernel check proves our child already exited,
/// before any compiler/source input. Nonblocking bounded reads cannot be held
/// open by a descendant. Raw bytes are neither printed nor persisted.
#[cfg(test)]
pub(super) fn early_exit(child: &mut std::process::Child) {
    #[cfg(debug_assertions)]
    {
        if !enabled() {
            return;
        }
        let Some(stderr) = child.stderr.as_mut() else {
            stage("unknown_early_exit_stderr");
            return;
        };
        let ready = rustix::fs::fcntl_getfl(&*stderr).and_then(|flags| {
            rustix::fs::fcntl_setfl(&*stderr, flags | rustix::fs::OFlags::NONBLOCK)
        });
        if ready.is_err() {
            stage("unknown_early_exit_stderr");
            return;
        }
        let mut bytes = [0; 4096];
        let count = stderr.read(&mut bytes).unwrap_or(0);
        stage(classify(&bytes[..count]));
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = child;
    }
}
#[cfg(all(test, debug_assertions))]
fn classify(bytes: &[u8]) -> &'static str {
    let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|part| part == needle);
    if contains(b"sandbox-exec: sandbox_apply: Operation not permitted") {
        "nested_sandbox_apply_denied"
    } else if contains(b"sandbox-exec: execvp") {
        "worker_exec_failed"
    } else if contains(b"dyld:") || contains(b"dyld[") {
        "worker_loader_failed"
    } else {
        "unknown_early_exit_stderr"
    }
}
#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    #[test]
    fn stderr_maps_only_fixed_categories_and_never_returns_child_text() {
        assert_eq!(
            classify(b"sandbox-exec: sandbox_apply: Operation not permitted\n"),
            "nested_sandbox_apply_denied"
        );
        assert_eq!(
            classify(b"sandbox-exec: execvp() of '/untrusted/path' failed: secret"),
            "worker_exec_failed"
        );
        assert_eq!(
            classify(b"dyld[123]: arbitrary details"),
            "worker_loader_failed"
        );
        for bytes in [
            b"secret arbitrary child stderr".as_slice(),
            b"sandbox_apply: Operation not permitted",
            b"",
            &[0xff, 0],
        ] {
            assert_eq!(classify(bytes), "unknown_early_exit_stderr");
        }
    }
}
