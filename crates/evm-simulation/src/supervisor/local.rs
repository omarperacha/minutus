//! Unqualified developer diagnostics retain the mandatory local process policy.
use crate::SimulationError;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
};
const SANDBOX: &str = r#"(version 1)
(deny default)
(allow process-exec (literal (param "WORKER")))
(allow file-read-metadata)
; Darwin loader opens the root directory; this literal grants no child files.
(allow file-read-data (literal "/"))
(allow file-read-data (literal (param "WORKER")) (subpath "/System/Library") (subpath "/usr/lib"))
(allow sysctl-read)
(allow process-info* (target self))
(allow signal (target self))
"#;

pub(super) fn launch(program: &Path, offline: bool) -> Result<ReapedChild, SimulationError> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (program, offline);
        Err(SimulationError::WorkerUnavailable)
    }
    #[cfg(target_os = "macos")]
    {
        let program = program
            .canonicalize()
            .map_err(|_| SimulationError::WorkerUnavailable)?;
        // OS sandbox is mandatory, not an opportunistic best-effort fallback.
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .args(["-p", SANDBOX, "-D"])
            .arg(format!(
                "WORKER={}",
                program.to_str().ok_or(SimulationError::WorkerUnavailable)?
            ))
            .arg(program)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let guardian = crate::native_resources::guardian::configure(&mut command)?;
        let child = command
            .spawn()
            .map_err(|_| SimulationError::WorkerUnavailable)?;
        Ok(ReapedChild {
            child,
            _guardian: guardian,
            offline,
        })
    }
}

// Own the exact unreaped process even on early pipe setup errors/unwinding.
pub(super) struct ReapedChild {
    child: Child,
    _guardian: std::io::PipeWriter,
    offline: bool,
}
impl std::ops::Deref for ReapedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}
impl std::ops::DerefMut for ReapedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}
impl ReapedChild {
    pub(super) fn reap(&mut self) -> Result<(), SimulationError> {
        let _ = self.child.kill();
        if self.child.wait().is_err() {
            if self.offline {
                super::control::quarantine();
            }
            return Err(SimulationError::WorkerUnavailable);
        }
        Ok(())
    }
}
impl Drop for ReapedChild {
    fn drop(&mut self) {
        let _ = self.reap();
    }
}
