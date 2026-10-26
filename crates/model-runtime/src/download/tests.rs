use std::{collections::VecDeque, io::Cursor, sync::Mutex};

use serde_json::json;
use tempfile::TempDir;

use crate::ModelCatalogue;

use super::*;

struct FakeSource {
    responses: Mutex<VecDeque<SourceResponse>>,
    offsets: Mutex<Vec<u64>>,
}

impl FakeSource {
    fn new(responses: Vec<SourceResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            offsets: Mutex::new(Vec::new()),
        }
    }

    fn offsets(&self) -> Result<Vec<u64>, io::Error> {
        self.offsets
            .lock()
            .map(|offsets| offsets.clone())
            .map_err(|_| io::Error::other("fake source offsets lock failed"))
    }
}

impl DownloadSource for FakeSource {
    fn open(&self, _url: &Url, offset: u64) -> Result<SourceResponse, DownloadError> {
        self.offsets
            .lock()
            .map_err(|_| DownloadError::Unavailable)?
            .push(offset);
        self.responses
            .lock()
            .map_err(|_| DownloadError::Unavailable)?
            .pop_front()
            .ok_or(DownloadError::HttpStatus(599))
    }
}

struct InterruptedReader {
    bytes: Cursor<Vec<u8>>,
    interrupted: bool,
}

impl Read for InterruptedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.bytes.read(buffer)?;
        if read > 0 {
            return Ok(read);
        }
        if self.interrupted {
            Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "test interruption",
            ))
        } else {
            Ok(0)
        }
    }
}

fn artifact(bytes: &[u8]) -> Result<ModelArtifact, Box<dyn Error>> {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let document = json!({
        "schemaVersion": crate::CATALOGUE_SCHEMA_VERSION,
        "models": [{
            "id": "test/example",
            "version": "revision-1",
            "displayName": "Test model",
            "integrationStatus": "integrated",
            "capabilities": {
                "features": ["textInput"],
                "contextLimitTokens": 16
            },
            "artifact": {
                "id": "primary",
                "fileName": "test.gguf",
                "format": "gguf",
                "downloadUrl": "https://example.test/model.gguf",
                "downloadBytes": bytes.len(),
                "installedBytes": bytes.len(),
                "sha256": sha256
            },
            "recommendedMemoryBytes": bytes.len(),
            "defaults": { "contextTokens": 16, "outputTokens": 4 },
            "backend": {
                "kind": "llamaCpp",
                "compatibilityVersion": "test-revision"
            },
            "companionCompatibilityVersion": "revision-1",
            "companions": [],
            "multimodalProfile": null,
            "licenseUrl": "https://example.test/license",
            "usagePolicyUrl": "https://example.test/policy"
        }]
    });
    let catalogue = ModelCatalogue::parse_json(&document.to_string())?;
    catalogue
        .models()
        .first()
        .map(|model| model.artifact().clone())
        .ok_or_else(|| io::Error::other("test catalogue has no artifact").into())
}

fn full_response(bytes: Vec<u8>) -> SourceResponse {
    SourceResponse {
        kind: ResponseKind::Full,
        content_length: u64::try_from(bytes.len()).ok(),
        reader: Box::new(Cursor::new(bytes)),
    }
}

fn partial_response(bytes: Vec<u8>, start: u64, total: u64) -> SourceResponse {
    SourceResponse {
        kind: ResponseKind::Partial { start, total },
        content_length: u64::try_from(bytes.len()).ok(),
        reader: Box::new(Cursor::new(bytes)),
    }
}

#[test]
fn complete_download_reports_remaining_and_publishes_atomically() -> Result<(), Box<dyn Error>> {
    let bytes = b"verified model bytes";
    let artifact = artifact(bytes)?;
    let directory = TempDir::new()?;
    let manager = ModelDownloadManager::initialize(directory.path().join("models"))?;
    let source = FakeSource::new(vec![full_response(bytes.to_vec())]);
    let mut snapshots = Vec::new();

    let installed = manager.download_with(
        &artifact,
        &DownloadCancellation::new(),
        |snapshot| snapshots.push(snapshot),
        &source,
        |_| Ok(u64::MAX),
    )?;

    assert!(installed.path().is_file());
    assert!(manager.is_published(&artifact, installed.path()));
    assert!(!manager.partial_path(&artifact).exists());
    assert_eq!(fs::read(installed.path())?, bytes);
    assert_eq!(installed.sha256(), artifact.sha256());
    assert_eq!(
        snapshots.first().map(|item| item.remaining_bytes()),
        Some(20)
    );
    assert_eq!(snapshots.last().map(|item| item.remaining_bytes()), Some(0));
    assert_eq!(
        snapshots
            .iter()
            .map(|item| item.phase())
            .collect::<Vec<_>>(),
        vec![
            DownloadPhase::Downloading,
            DownloadPhase::Downloading,
            DownloadPhase::Verifying,
            DownloadPhase::Installing,
        ]
    );

    let no_network = FakeSource::new(Vec::new());
    let repaired = manager.download_with(
        &artifact,
        &DownloadCancellation::new(),
        |_| {},
        &no_network,
        |_| Ok(0),
    )?;
    assert_eq!(repaired, installed);
    assert!(no_network.offsets()?.is_empty());
    Ok(())
}

#[test]
fn interrupted_download_survives_restart_and_resumes_at_exact_offset() -> Result<(), Box<dyn Error>>
{
    let bytes = b"resume-this-download";
    let artifact = artifact(bytes)?;
    let directory = TempDir::new()?;
    let root = directory.path().join("models");
    let manager = ModelDownloadManager::initialize(&root)?;
    let retained = bytes[..7].to_vec();
    let interrupted = FakeSource::new(vec![SourceResponse {
        kind: ResponseKind::Full,
        content_length: Some(u64::try_from(bytes.len())?),
        reader: Box::new(InterruptedReader {
            bytes: Cursor::new(retained.clone()),
            interrupted: true,
        }),
    }]);
    let first = manager.download_with(
        &artifact,
        &DownloadCancellation::new(),
        |_| {},
        &interrupted,
        |_| Ok(u64::MAX),
    );
    assert!(matches!(first, Err(DownloadError::Transfer(_))));
    assert_eq!(fs::read(manager.partial_path(&artifact))?, retained);
    drop(manager);

    let restarted = ModelDownloadManager::initialize(&root)?;
    let resume = FakeSource::new(vec![partial_response(
        bytes[7..].to_vec(),
        7,
        u64::try_from(bytes.len())?,
    )]);
    let mut snapshots = Vec::new();
    let installed = restarted.download_with(
        &artifact,
        &DownloadCancellation::new(),
        |snapshot| snapshots.push(snapshot),
        &resume,
        |_| Ok(u64::MAX),
    )?;

    assert_eq!(resume.offsets()?, vec![7]);
    assert_eq!(fs::read(installed.path())?, bytes);
    assert_eq!(snapshots.first().map(|item| item.resumed()), Some(true));
    assert_eq!(
        snapshots.first().map(|item| item.phase()),
        Some(DownloadPhase::Downloading)
    );
    assert_eq!(
        snapshots.last().map(|item| item.phase()),
        Some(DownloadPhase::Installing)
    );
    Ok(())
}

#[test]
fn server_without_range_support_restarts_from_zero_safely() -> Result<(), Box<dyn Error>> {
    let bytes = b"server sends a complete body";
    let artifact = artifact(bytes)?;
    let directory = TempDir::new()?;
    let manager = ModelDownloadManager::initialize(directory.path().join("models"))?;
    fs::write(manager.partial_path(&artifact), &bytes[..6])?;
    let source = FakeSource::new(vec![full_response(bytes.to_vec())]);

    let installed = manager.download_with(
        &artifact,
        &DownloadCancellation::new(),
        |_| {},
        &source,
        |_| Ok(u64::MAX),
    )?;

    assert_eq!(source.offsets()?, vec![6]);
    assert_eq!(fs::read(installed.path())?, bytes);
    Ok(())
}

#[test]
fn disk_preflight_and_cancellation_leave_no_false_install() -> Result<(), Box<dyn Error>> {
    let bytes = b"disk and cancellation checks";
    let artifact = artifact(bytes)?;
    let directory = TempDir::new()?;
    let manager = ModelDownloadManager::initialize(directory.path().join("models"))?;
    let source = FakeSource::new(Vec::new());
    let result = manager.download_with(
        &artifact,
        &DownloadCancellation::new(),
        |_| {},
        &source,
        |_| Ok(1),
    );
    assert!(matches!(
        result,
        Err(DownloadError::InsufficientDiskSpace { .. })
    ));
    assert!(source.offsets()?.is_empty());
    assert!(!manager.installed_path(&artifact).exists());

    fs::write(manager.partial_path(&artifact), &bytes[..5])?;
    let cancellation = DownloadCancellation::new();
    cancellation.cancel();
    let cancelled =
        manager.download_with(&artifact, &cancellation, |_| {}, &source, |_| Ok(u64::MAX));
    assert!(matches!(cancelled, Err(DownloadError::Cancelled)));
    assert_eq!(fs::read(manager.partial_path(&artifact))?, &bytes[..5]);
    assert!(!manager.installed_path(&artifact).exists());
    Ok(())
}

#[test]
fn aggregate_preflight_deducts_published_and_partial_artifacts() -> Result<(), Box<dyn Error>> {
    let primary = artifact(b"primary artifact")?;
    let companion = artifact(b"companion artifact bytes")?;
    let directory = TempDir::new()?;
    let manager = ModelDownloadManager::initialize(directory.path().join("models"))?;
    fs::write(
        manager.installed_path(&primary),
        vec![0_u8; usize::try_from(primary.installed_bytes())?],
    )?;
    fs::write(manager.partial_path(&companion), b"retained")?;
    let expected = companion.download_bytes() - 8;
    assert_eq!(
        manager.ensure_disk_space_for_with(&[&primary, &companion], |_| Ok(expected))?,
        expected
    );
    assert!(matches!(
        manager.ensure_disk_space_for_with(&[&primary, &companion], |_| Ok(expected - 1)),
        Err(DownloadError::InsufficientDiskSpace { .. })
    ));
    Ok(())
}

#[test]
fn checksum_failure_is_rejected_and_cleared_for_retry() -> Result<(), Box<dyn Error>> {
    let expected = b"expected artifact";
    let wrong = b"wrong----artifact";
    assert_eq!(expected.len(), wrong.len());
    let artifact = artifact(expected)?;
    let directory = TempDir::new()?;
    let manager = ModelDownloadManager::initialize(directory.path().join("models"))?;
    let source = FakeSource::new(vec![full_response(wrong.to_vec())]);

    let result = manager.download_with(
        &artifact,
        &DownloadCancellation::new(),
        |_| {},
        &source,
        |_| Ok(u64::MAX),
    );

    assert!(matches!(
        result,
        Err(DownloadError::ChecksumMismatch { .. })
    ));
    assert_eq!(fs::metadata(manager.partial_path(&artifact))?.len(), 0);
    assert!(!manager.installed_path(&artifact).exists());
    Ok(())
}

#[test]
fn invalid_range_is_rejected_without_appending() -> Result<(), Box<dyn Error>> {
    let bytes = b"range validation";
    let artifact = artifact(bytes)?;
    let directory = TempDir::new()?;
    let manager = ModelDownloadManager::initialize(directory.path().join("models"))?;
    fs::write(manager.partial_path(&artifact), &bytes[..4])?;
    let source = FakeSource::new(vec![partial_response(
        bytes[4..].to_vec(),
        3,
        u64::try_from(bytes.len())?,
    )]);

    let result = manager.download_with(
        &artifact,
        &DownloadCancellation::new(),
        |_| {},
        &source,
        |_| Ok(u64::MAX),
    );
    assert!(matches!(
        result,
        Err(DownloadError::InvalidRangeResponse { .. })
    ));
    assert_eq!(fs::read(manager.partial_path(&artifact))?, &bytes[..4]);
    Ok(())
}

#[test]
fn content_range_parser_is_strict() {
    assert!(matches!(parse_content_range("bytes 5-9/10"), Ok((5, 10))));
    assert!(parse_content_range("items 5-9/10").is_err());
    assert!(parse_content_range("bytes 10-9/10").is_err());
    assert!(parse_content_range("bytes 5-10/10").is_err());
}
