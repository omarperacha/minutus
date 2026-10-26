//! Shared reads and exclusive capability changes for workspace operations.

use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::AppCoreError;

pub(crate) struct WorkspaceOperationGate {
    lock: RwLock<()>,
}

impl WorkspaceOperationGate {
    pub(crate) const fn new() -> Self {
        Self {
            lock: RwLock::new(()),
        }
    }

    pub(super) fn read(&self) -> Result<RwLockReadGuard<'_, ()>, AppCoreError> {
        self.lock
            .read()
            .map_err(|_| AppCoreError::WorkspaceOperationsUnavailable)
    }

    pub(super) fn write(&self) -> Result<RwLockWriteGuard<'_, ()>, AppCoreError> {
        self.lock
            .write()
            .map_err(|_| AppCoreError::WorkspaceOperationsUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_share_the_gate_while_mutations_remain_exclusive() -> Result<(), AppCoreError> {
        let gate = WorkspaceOperationGate::new();
        let first = gate.read()?;
        let second = gate.read()?;
        assert!(gate.lock.try_write().is_err());
        drop((first, second));
        assert!(gate.lock.try_write().is_ok());
        Ok(())
    }
}
