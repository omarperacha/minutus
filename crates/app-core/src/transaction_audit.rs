//! Composition API for public transaction auditing and explicit export.

use std::path::Path;

use audit_log::{
    LogWriteOutcome, TransactionAuditDeleteReceipt, TransactionAuditEvent,
    TransactionAuditExportPlan, TransactionAuditExportReceipt,
};

use crate::{AppCore, AppCoreError};

impl AppCore {
    /// Best-effort appends one validated public transaction audit event.
    ///
    /// Audit outcome is deliberately not returned as transaction authority.
    /// Callers must preserve their independently validated lifecycle result when
    /// this returns `Failed`.
    #[must_use]
    pub fn record_transaction_audit(&self, event: TransactionAuditEvent) -> LogWriteOutcome {
        self.transaction_audit.record(event)
    }

    /// Snapshots the bounded transaction audit for mandatory user preview.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error if retained audit files are corrupt,
    /// unavailable or exceed the fixed export budget.
    pub fn plan_transaction_audit_export(
        &self,
    ) -> Result<TransactionAuditExportPlan, AppCoreError> {
        self.transaction_audit.plan_export().map_err(Into::into)
    }

    /// Writes a previously previewed transaction audit snapshot without overwrite.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error if the private destination cannot be created
    /// and fully written.
    pub fn write_transaction_audit_export(
        &self,
        plan: TransactionAuditExportPlan,
        destination: impl AsRef<Path>,
    ) -> Result<TransactionAuditExportReceipt, AppCoreError> {
        plan.write_to(destination).map_err(Into::into)
    }

    /// Explicitly deletes every active and retained transaction audit file.
    ///
    /// # Errors
    ///
    /// Returns a sanitized local error. Partial deletion remains safe to retry.
    pub fn delete_transaction_audit(&self) -> Result<TransactionAuditDeleteReceipt, AppCoreError> {
        self.transaction_audit.delete_all().map_err(Into::into)
    }
}
