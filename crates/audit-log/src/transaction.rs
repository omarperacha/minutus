//! Public-metadata transaction audit log with explicit export disclosure.

use std::{
    collections::BTreeSet,
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};

use crate::{
    LogWriteOutcome, MAX_LOG_FILE_BYTES, RETAINED_LOG_FILES,
    logger::{
        prepare_private_directory, private_append_file, private_create_new, remove_if_present,
        rotate_if_needed, rotated_path, timestamp_unix_ms,
    },
};

/// Version of the transaction audit JSON Lines record schema.
pub(crate) const TRANSACTION_AUDIT_SCHEMA_VERSION: u16 = 1;

const ACTIVE_LOG_NAME: &str = "zeta-transaction-audit.jsonl";
const EXPORT_SCHEMA_VERSION: u16 = 1;
const MAX_ID_BYTES: usize = 128;
const MAX_WARNING_CODES: usize = 16;
const PRIVACY_DESCRIPTION: &str = "Minutus transaction audit events in JSON Lines format. This export includes public wallet addresses, chain IDs, signing or transaction digests, transaction decisions, warning codes and lifecycle outcomes. It excludes private keys, recovery phrases, seed material, vault plaintext, Keychain authentication data, signatures, signed transaction bytes, signing payloads, prompts and model explanations.";

/// Closed trusted origin classification without request prose.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionAuditOrigin {
    /// Direct user action in the native application.
    User,
    /// Agent-initiated request routed through trusted host policy.
    Agent,
    /// Trusted x402 payment flow.
    X402,
    /// Internal host recovery or lifecycle coordination.
    LocalSystem,
}

/// Closed transaction/signing domain recorded publicly.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionAuditTransactionType {
    /// EIP-1559 EIP-2718 type-2 transaction.
    Eip1559,
    /// Supported EIP-712 structured-data authorization.
    Eip712,
}

/// Closed transaction lifecycle state for audit reconstruction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionAuditState {
    /// Candidate proposed.
    Proposed,
    /// Candidate validated.
    Validated,
    /// Candidate simulated.
    Simulated,
    /// Native approval pending.
    AwaitingApproval,
    /// Exact candidate approved.
    Approved,
    /// Signature produced.
    Signed,
    /// Exact signed transaction accepted by a node.
    Broadcast,
    /// Successful receipt confirmed.
    Confirmed,
    /// Terminal failure.
    Failed,
}

/// Bounded user decision returned by the trusted approval surface.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAuditDecision {
    /// User approved the exact displayed transaction.
    Approved,
    /// User explicitly denied it.
    Denied,
    /// User dismissed the native surface.
    Dismissed,
    /// Trusted host cancelled the request.
    Cancelled,
    /// Exact approval expired.
    Expired,
}

/// Closed operating-system owner authorization result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerAuthorizationAuditOutcome {
    /// OS-backed owner authorization succeeded.
    Succeeded,
    /// OS-backed owner authorization was denied or locked out.
    Denied,
    /// User cancelled the operating-system prompt.
    Cancelled,
    /// Required operating-system authorization was unavailable.
    Unavailable,
}

/// Closed durable approval finalization result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalFinalizationAuditOutcome {
    /// Exact approval was atomically consumed.
    Consumed,
    /// Exact approval was terminally invalidated.
    Invalidated,
    /// Replay of an already consumed approval was rejected.
    ReplayRejected,
    /// Random identifier collision or rebinding was rejected.
    IdentifierCollision,
    /// Durable finalization storage was unavailable.
    StorageUnavailable,
}

/// Closed warning codes derived by trusted decoding, simulation and policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionAuditWarningCode {
    /// Exact debug-only local Flowzed emulator endpoint.
    LocalDevelopmentEndpoint,
    /// Selected network is a testnet.
    Testnet,
    /// Chain identity mismatch.
    ChainMismatch,
    /// Contract is not in trusted configuration.
    UnknownContract,
    /// Calldata selector is unknown.
    UnknownSelector,
    /// Token allowance is unlimited.
    UnlimitedApproval,
    /// Token allowance is effectively unlimited under bounded policy.
    EffectivelyUnlimitedApproval,
    /// Spender is suspicious under trusted policy.
    SuspiciousSpender,
    /// Spender is not in trusted configuration.
    UnrecognizedSpender,
    /// x402 recipient is not in trusted configuration.
    UnrecognizedRecipient,
    /// x402 atomic amount exceeds the trusted review threshold.
    UnusualPrice,
    /// Token action also moves unexpected native value.
    UnexpectedNativeValue,
    /// Typed authorization owner differs from the selected account.
    OwnerMismatch,
    /// Typed authorization domain is not recognized.
    UnrecognizedDomain,
    /// Authorization deadline has passed.
    ExpiredAuthorization,
    /// Authorization deadline is close.
    ExpiringAuthorization,
    /// Typed data is outside the supported closed schema.
    UnsupportedTypedData,
    /// Security-relevant authorization field could not be decoded.
    UndecodableField,
    /// Simulation reverted.
    SimulationReverted,
    /// Simulation was unavailable.
    SimulationUnavailable,
    /// Simulation was inconclusive.
    SimulationInconclusive,
    /// Simulation disagreed with the candidate.
    SimulationDiscrepancy,
    /// Simulation evidence was stale.
    SimulationStale,
}

/// Bounded failure codes with no arbitrary error text.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionAuditFailureCode {
    /// Candidate validation failed.
    ValidationFailed,
    /// Simulation failed closed.
    SimulationFailed,
    /// Approval preparation failed.
    ApprovalPreparationFailed,
    /// Exact candidate changed after approval.
    CandidateInvalidated,
    /// OS owner authorization failed.
    OwnerAuthorizationFailed,
    /// Approval was expired, invalidated or replayed.
    ApprovalUnavailable,
    /// Wallet signing failed.
    SigningFailed,
    /// Broadcast validation, transport or node submission failed.
    BroadcastFailed,
    /// Receipt established an on-chain failure.
    ConfirmationFailed,
    /// A fresh x402 challenge replaced the approved requirement.
    X402RequirementChanged,
    /// The x402 paid request failed before submission could begin.
    X402RetryFailed,
    /// A signature-bearing request may have reached the payment service.
    X402OutcomeAmbiguous,
    /// Returned x402 settlement evidence failed validation.
    X402SettlementFailed,
    /// Settlement succeeded but purchased content could not be delivered.
    X402ContentFailed,
    /// Application restart invalidated non-durable work.
    ApplicationRestarted,
    /// Audit persistence itself was unavailable.
    AuditUnavailable,
    /// Trusted internal invariant failed closed.
    InternalInvariant,
}

/// Closed x402 stage names for public, content-free audit records.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum X402AuditStage {
    /// An exact request-bound payment candidate was prepared.
    Preparation,
    /// Trusted native review resolved.
    Approval,
    /// Owner-present wallet signing resolved.
    Signing,
    /// The one allowed signature-bearing HTTP request resolved.
    PaidRetry,
    /// Returned settlement evidence was validated.
    SettlementValidation,
    /// Bounded untrusted purchased content crossed the extraction boundary.
    ContentDelivery,
}

/// Closed success/failure result for effect stages.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "failureCode")]
pub enum TransactionAuditResult {
    /// Stage completed successfully.
    Succeeded,
    /// Stage failed with a bounded trusted code.
    Failed(TransactionAuditFailureCode),
}

/// Distinguishable transaction audit event stages.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TransactionAuditEventKind {
    /// Explicit trusted lifecycle transition.
    LifecycleTransition {
        /// Previous state, absent only for proposal.
        from: Option<TransactionAuditState>,
        /// Resulting state.
        to: TransactionAuditState,
        /// Failure code when the resulting state is failed.
        failure_code: Option<TransactionAuditFailureCode>,
    },
    /// Native approval surface displayed exact trusted facts.
    ApprovalPresented {
        /// Prominent trusted warning codes shown to the user.
        warning_codes: Vec<TransactionAuditWarningCode>,
    },
    /// Trusted native approval decision.
    ApprovalDecision {
        /// Exact decision from the native surface.
        decision: ApprovalAuditDecision,
    },
    /// Fresh OS-backed owner authorization result.
    OwnerAuthorization {
        /// Result returned by the OS-backed owner-authentication boundary.
        outcome: OwnerAuthorizationAuditOutcome,
    },
    /// Durable approval consumption, invalidation or replay result.
    ApprovalFinalization {
        /// Durable single-use finalization result.
        outcome: ApprovalFinalizationAuditOutcome,
    },
    /// Wallet signature result.
    Signing {
        /// Wallet signing result.
        result: TransactionAuditResult,
    },
    /// Signed-transaction submission result.
    Broadcast {
        /// Exact signed-transaction submission result.
        result: TransactionAuditResult,
    },
    /// Receipt-monitoring terminal result.
    Confirmation {
        /// Receipt-derived terminal result.
        result: TransactionAuditResult,
    },
    /// One bounded x402 payment stage completed or failed.
    X402Payment {
        /// Trusted stage identifier with no remote content.
        stage: X402AuditStage,
        /// Closed result and failure code.
        result: TransactionAuditResult,
        /// Whether the signature-bearing request may have caused payment.
        payment_may_have_occurred: bool,
    },
}

/// Validated public transaction audit event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransactionAuditEvent {
    correlation_id: String,
    transaction_id: String,
    public_account: String,
    chain_id: u64,
    transaction_type: TransactionAuditTransactionType,
    signing_digest: Option<String>,
    transaction_hash: Option<String>,
    origin: TransactionAuditOrigin,
    data: TransactionAuditEventKind,
}

impl TransactionAuditEvent {
    /// Validates one event made exclusively from trusted public metadata.
    ///
    /// Digest inputs are fixed byte arrays so raw payloads, signatures and
    /// secret strings cannot occupy their fields.
    ///
    /// # Errors
    ///
    /// Returns a closed error for malformed identifiers, accounts, chain IDs,
    /// excessive or duplicate warnings, or inconsistent lifecycle failures.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        correlation_id: &str,
        transaction_id: &str,
        public_account: &str,
        chain_id: u64,
        transaction_type: TransactionAuditTransactionType,
        signing_digest: Option<[u8; 32]>,
        transaction_hash: Option<[u8; 32]>,
        origin: TransactionAuditOrigin,
        data: TransactionAuditEventKind,
    ) -> Result<Self, TransactionAuditError> {
        if !valid_id(correlation_id) || !valid_id(transaction_id) {
            return Err(TransactionAuditError::InvalidEvent);
        }
        if !valid_evm_account(public_account) || chain_id == 0 {
            return Err(TransactionAuditError::InvalidEvent);
        }
        validate_kind(&data)?;
        Ok(Self {
            correlation_id: correlation_id.to_owned(),
            transaction_id: transaction_id.to_owned(),
            public_account: public_account.to_ascii_lowercase(),
            chain_id,
            transaction_type,
            signing_digest: signing_digest.map(hex),
            transaction_hash: transaction_hash.map(hex),
            origin,
            data,
        })
    }

    /// Returns the closed event stage.
    #[must_use]
    pub const fn kind(&self) -> &TransactionAuditEventKind {
        &self.data
    }
}

fn validate_kind(kind: &TransactionAuditEventKind) -> Result<(), TransactionAuditError> {
    match kind {
        TransactionAuditEventKind::LifecycleTransition {
            to, failure_code, ..
        } if (*to == TransactionAuditState::Failed) != failure_code.is_some() => {
            Err(TransactionAuditError::InvalidEvent)
        }
        TransactionAuditEventKind::ApprovalPresented { warning_codes } => {
            let unique = warning_codes.iter().copied().collect::<BTreeSet<_>>();
            if warning_codes.len() > MAX_WARNING_CODES || unique.len() != warning_codes.len() {
                Err(TransactionAuditError::InvalidEvent)
            } else {
                Ok(())
            }
        }
        TransactionAuditEventKind::X402Payment {
            result,
            payment_may_have_occurred,
            ..
        } => {
            let may_have_occurred = matches!(
                result,
                TransactionAuditResult::Failed(
                    TransactionAuditFailureCode::X402RequirementChanged
                        | TransactionAuditFailureCode::X402OutcomeAmbiguous
                        | TransactionAuditFailureCode::X402SettlementFailed
                )
            );
            if *payment_may_have_occurred == may_have_occurred {
                Ok(())
            } else {
                Err(TransactionAuditError::InvalidEvent)
            }
        }
        _ => Ok(()),
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransactionAuditRecord {
    schema_version: u16,
    timestamp_unix_ms: u64,
    sequence: u64,
    data: TransactionAuditEvent,
}

/// Sanitized transaction audit validation, export or deletion failure.
#[derive(Debug)]
pub enum TransactionAuditError {
    /// Trusted code supplied malformed public event metadata.
    InvalidEvent,
    /// Local audit files could not be read, removed or written.
    Io(std::io::Error),
    /// Audit logger state is temporarily unavailable.
    Unavailable,
    /// Local files exceeded the bounded export budget.
    SizeLimitExceeded,
    /// A local transaction audit file did not match the closed schema.
    CorruptSource,
    /// Export destination already exists and will not be overwritten.
    DestinationExists,
}

impl fmt::Display for TransactionAuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEvent => "transaction audit event is invalid",
            Self::Io(_) => "transaction audit operation could not be completed",
            Self::Unavailable => "transaction audit is temporarily unavailable",
            Self::SizeLimitExceeded => "transaction audit export exceeds its size limit",
            Self::CorruptSource => "local transaction audit is malformed",
            Self::DestinationExists => "transaction audit export destination already exists",
        })
    }
}

impl Error for TransactionAuditError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for TransactionAuditError {
    fn from(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Self::DestinationExists
        } else {
            Self::Io(error)
        }
    }
}

/// Thread-safe, bounded, always-on public transaction audit logger.
pub struct TransactionAuditLogger {
    directory: PathBuf,
    sequence: AtomicU64,
    write_lock: Mutex<()>,
}

impl TransactionAuditLogger {
    /// Configures the audit beneath a Rust-owned application-data directory.
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        let sequence = next_sequence(&directory);
        Self {
            directory,
            sequence: AtomicU64::new(sequence),
            write_lock: Mutex::new(()),
        }
    }

    /// Best-effort appends one validated transaction event.
    ///
    /// Failure is intentionally separate from transaction control flow: it can
    /// neither grant approval nor advance lifecycle or signing state.
    #[must_use]
    pub fn record(&self, event: TransactionAuditEvent) -> LogWriteOutcome {
        let Ok(_guard) = self.write_lock.lock() else {
            return LogWriteOutcome::Failed;
        };
        match self.write_record(event) {
            Ok(()) => LogWriteOutcome::Written,
            Err(()) => LogWriteOutcome::Failed,
        }
    }

    /// Snapshots the bounded audit for mandatory user preview before export.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error for unavailable, oversized or corrupt files.
    pub fn plan_export(&self) -> Result<TransactionAuditExportPlan, TransactionAuditError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| TransactionAuditError::Unavailable)?;
        build_export_plan(&self.directory)
    }

    /// Explicitly deletes all retained transaction audit files.
    ///
    /// # Errors
    ///
    /// Returns a sanitized local I/O error. A partial failure may leave some
    /// files retained and is safe to retry.
    pub fn delete_all(&self) -> Result<TransactionAuditDeleteReceipt, TransactionAuditError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| TransactionAuditError::Unavailable)?;
        let active = self.directory.join(ACTIVE_LOG_NAME);
        let mut deleted_files = 0_usize;
        for path in std::iter::once(active.clone())
            .chain((1..=RETAINED_LOG_FILES).map(|index| rotated_path(&active, index)))
        {
            if path.exists() {
                remove_if_present(&path)?;
                deleted_files += 1;
            }
        }
        Ok(TransactionAuditDeleteReceipt { deleted_files })
    }

    fn write_record(&self, event: TransactionAuditEvent) -> Result<(), ()> {
        validate_persisted_event(&event).map_err(|_| ())?;
        prepare_private_directory(&self.directory).map_err(|_| ())?;
        let sequence = self
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| ())?;
        let record = TransactionAuditRecord {
            schema_version: TRANSACTION_AUDIT_SCHEMA_VERSION,
            timestamp_unix_ms: timestamp_unix_ms(),
            sequence,
            data: event,
        };
        let mut encoded = serde_json::to_vec(&record).map_err(|_| ())?;
        encoded.push(b'\n');
        let active = self.directory.join(ACTIVE_LOG_NAME);
        rotate_if_needed(&active, encoded.len() as u64).map_err(|_| ())?;
        let mut file = private_append_file(&active).map_err(|_| ())?;
        file.write_all(&encoded).map_err(|_| ())?;
        file.flush().map_err(|_| ())
    }
}

fn next_sequence(directory: &Path) -> u64 {
    let active = directory.join(ACTIVE_LOG_NAME);
    let mut paths = (1..=RETAINED_LOG_FILES)
        .map(|index| rotated_path(&active, index))
        .collect::<Vec<_>>();
    paths.push(active);
    paths
        .into_iter()
        .filter_map(|path| fs::read(path).ok())
        .flat_map(|source| {
            source
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .filter_map(|line| serde_json::from_slice::<TransactionAuditRecord>(line).ok())
                .map(|record| record.sequence)
                .collect::<Vec<_>>()
        })
        .max()
        .and_then(|value| value.checked_add(1))
        .unwrap_or(0)
}

/// Mandatory preview of public metadata included in the exact export snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransactionAuditExportPreview {
    source_file_count: usize,
    record_count: usize,
    content_bytes: u64,
}

impl TransactionAuditExportPreview {
    /// Number of retained audit files represented by the snapshot.
    #[must_use]
    pub const fn source_file_count(&self) -> usize {
        self.source_file_count
    }
    /// Number of transaction events represented by the snapshot.
    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.record_count
    }
    /// Total audit bytes before the export manifest.
    #[must_use]
    pub const fn content_bytes(&self) -> u64 {
        self.content_bytes
    }
    /// Static disclosure of public on-chain metadata included in the export.
    #[must_use]
    pub const fn privacy_description(&self) -> &'static str {
        PRIVACY_DESCRIPTION
    }
}

/// Exact bounded transaction audit snapshot prepared for explicit export.
pub struct TransactionAuditExportPlan {
    preview: TransactionAuditExportPreview,
    files: Vec<Vec<u8>>,
}

impl TransactionAuditExportPlan {
    /// Returns the mandatory public-metadata export preview.
    #[must_use]
    pub const fn preview(&self) -> &TransactionAuditExportPreview {
        &self.preview
    }

    /// Writes this exact snapshot to a new private local file.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error and never overwrites an existing destination.
    pub fn write_to(
        self,
        destination: impl AsRef<Path>,
    ) -> Result<TransactionAuditExportReceipt, TransactionAuditError> {
        let mut destination = private_create_new(destination.as_ref())?;
        let manifest = TransactionExportManifest {
            export_schema_version: EXPORT_SCHEMA_VERSION,
            transaction_audit_schema_version: TRANSACTION_AUDIT_SCHEMA_VERSION,
            source_file_count: self.preview.source_file_count,
            record_count: self.preview.record_count,
            content_bytes: self.preview.content_bytes,
            privacy: PRIVACY_DESCRIPTION,
        };
        let mut bytes_written = write_json_line(&mut destination, &manifest)?;
        for bytes in self.files {
            destination.write_all(&bytes)?;
            bytes_written = bytes_written.saturating_add(bytes.len() as u64);
        }
        destination.flush()?;
        Ok(TransactionAuditExportReceipt { bytes_written })
    }
}

/// Receipt for one explicit transaction audit export.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransactionAuditExportReceipt {
    bytes_written: u64,
}

impl TransactionAuditExportReceipt {
    /// Total manifest and event bytes written.
    #[must_use]
    pub const fn bytes_written(self) -> u64 {
        self.bytes_written
    }
}

/// Receipt for explicit deletion of retained audit files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransactionAuditDeleteReceipt {
    deleted_files: usize,
}

impl TransactionAuditDeleteReceipt {
    /// Number of active or rotated files removed.
    #[must_use]
    pub const fn deleted_files(self) -> usize {
        self.deleted_files
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransactionExportManifest {
    export_schema_version: u16,
    transaction_audit_schema_version: u16,
    source_file_count: usize,
    record_count: usize,
    content_bytes: u64,
    privacy: &'static str,
}

fn build_export_plan(
    directory: &Path,
) -> Result<TransactionAuditExportPlan, TransactionAuditError> {
    let active = directory.join(ACTIVE_LOG_NAME);
    let mut paths = (1..=RETAINED_LOG_FILES)
        .rev()
        .map(|index| rotated_path(&active, index))
        .collect::<Vec<_>>();
    paths.push(active);
    let maximum = MAX_LOG_FILE_BYTES.saturating_mul(u64::from(RETAINED_LOG_FILES) + 1);
    let mut files = Vec::new();
    let mut content_bytes = 0_u64;
    let mut record_count = 0_usize;
    for path in paths {
        let source = match fs::read(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let canonical = validate_and_canonicalize(&source)?;
        content_bytes = content_bytes.saturating_add(canonical.len() as u64);
        if content_bytes > maximum {
            return Err(TransactionAuditError::SizeLimitExceeded);
        }
        record_count += canonical
            .split(|byte| *byte == b'\n')
            .count()
            .saturating_sub(1);
        files.push(canonical);
    }
    Ok(TransactionAuditExportPlan {
        preview: TransactionAuditExportPreview {
            source_file_count: files.len(),
            record_count,
            content_bytes,
        },
        files,
    })
}

fn validate_and_canonicalize(source: &[u8]) -> Result<Vec<u8>, TransactionAuditError> {
    let mut canonical = Vec::new();
    for line in source
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let record: TransactionAuditRecord =
            serde_json::from_slice(line).map_err(|_| TransactionAuditError::CorruptSource)?;
        if record.schema_version != TRANSACTION_AUDIT_SCHEMA_VERSION {
            return Err(TransactionAuditError::CorruptSource);
        }
        validate_persisted_event(&record.data).map_err(|_| TransactionAuditError::CorruptSource)?;
        serde_json::to_writer(&mut canonical, &record)
            .map_err(|_| TransactionAuditError::Unavailable)?;
        canonical.push(b'\n');
    }
    Ok(canonical)
}

fn validate_persisted_event(event: &TransactionAuditEvent) -> Result<(), TransactionAuditError> {
    if !valid_id(&event.correlation_id)
        || !valid_id(&event.transaction_id)
        || !valid_evm_account(&event.public_account)
        || event.public_account != event.public_account.to_ascii_lowercase()
        || event.chain_id == 0
        || event
            .signing_digest
            .as_deref()
            .is_some_and(|value| !valid_hash(value))
        || event
            .transaction_hash
            .as_deref()
            .is_some_and(|value| !valid_hash(value))
    {
        return Err(TransactionAuditError::InvalidEvent);
    }
    validate_kind(&event.data)
}

fn write_json_line(
    file: &mut fs::File,
    value: &impl Serialize,
) -> Result<u64, TransactionAuditError> {
    let mut encoded = serde_json::to_vec(value).map_err(|_| TransactionAuditError::Unavailable)?;
    encoded.push(b'\n');
    file.write_all(&encoded)?;
    Ok(encoded.len() as u64)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_evm_account(value: &str) -> bool {
    value.len() == 42
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_hash(value: &str) -> bool {
    value.len() == 66
        && value.starts_with("0x")
        && value[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn hex(bytes: [u8; 32]) -> String {
    use fmt::Write as _;
    let mut encoded = String::with_capacity(66);
    encoded.push_str("0x");
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests;
