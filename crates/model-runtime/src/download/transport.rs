use super::{
    ACCEPT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, COPY_BUFFER_BYTES, Client, Digest,
    DownloadCancellation, DownloadError, DownloadPhase, DownloadProgress, File, ModelArtifact,
    OpenOptions, PROGRESS_INTERVAL_BYTES, Path, RANGE, Read, Seek, SeekFrom, Sha256, StatusCode,
    Url, VerifiedModelArtifact, Write, fs, io,
};

pub(super) fn transfer_response(
    artifact: &ModelArtifact,
    cancellation: &DownloadCancellation,
    progress: &mut impl FnMut(DownloadProgress),
    partial: &mut File,
    response: SourceResponse,
    offset: &mut u64,
    resumed: bool,
) -> Result<u64, DownloadError> {
    match response.kind {
        ResponseKind::Full => {
            if *offset > 0 {
                reset_partial(partial)?;
                *offset = 0;
            }
        }
        ResponseKind::Partial { start, total } => {
            if start != *offset || total != artifact.download_bytes() {
                return Err(DownloadError::InvalidRangeResponse {
                    requested_start: *offset,
                    received_start: start,
                    received_total: total,
                });
            }
        }
    }
    let remaining = artifact.download_bytes().saturating_sub(*offset);
    if response
        .content_length
        .is_some_and(|length| length != remaining)
    {
        return Err(DownloadError::UnexpectedContentLength {
            expected: remaining,
            received: response.content_length.unwrap_or_default(),
        });
    }

    partial
        .seek(SeekFrom::Start(*offset))
        .map_err(DownloadError::Storage)?;
    let mut reader = response.reader;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut downloaded = *offset;
    let mut last_reported = downloaded;
    loop {
        check_cancelled(cancellation)?;
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => {
                check_cancelled(cancellation)?;
                partial.sync_all().map_err(DownloadError::Storage)?;
                return Err(DownloadError::Transfer(error));
            }
        };
        if read == 0 {
            break;
        }
        let read = u64::try_from(read).map_err(|_| DownloadError::ArtifactTooLarge)?;
        downloaded = downloaded
            .checked_add(read)
            .ok_or(DownloadError::ArtifactTooLarge)?;
        if downloaded > artifact.download_bytes() {
            reset_partial(partial)?;
            return Err(DownloadError::ArtifactTooLarge);
        }
        let read = usize::try_from(read).map_err(|_| DownloadError::ArtifactTooLarge)?;
        partial
            .write_all(&buffer[..read])
            .map_err(DownloadError::Storage)?;
        if downloaded == artifact.download_bytes()
            || downloaded.saturating_sub(last_reported) >= PROGRESS_INTERVAL_BYTES
        {
            progress(DownloadProgress {
                downloaded_bytes: downloaded,
                total_bytes: artifact.download_bytes(),
                resumed,
                phase: DownloadPhase::Downloading,
            });
            last_reported = downloaded;
        }
    }
    Ok(downloaded)
}

pub(super) struct ReqwestSource<'a> {
    pub(super) client: &'a Client,
}

impl DownloadSource for ReqwestSource<'_> {
    fn open(&self, url: &Url, offset: u64) -> Result<SourceResponse, DownloadError> {
        let mut request = self
            .client
            .get(url.clone())
            .header(ACCEPT_ENCODING, "identity");
        if offset > 0 {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        let response = request.send().map_err(DownloadError::Network)?;
        let status = response.status();
        let kind = if status == StatusCode::OK {
            ResponseKind::Full
        } else if status == StatusCode::PARTIAL_CONTENT {
            let value = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .ok_or(DownloadError::MissingContentRange)?;
            let (start, total) = parse_content_range(value)?;
            ResponseKind::Partial { start, total }
        } else {
            return Err(DownloadError::HttpStatus(status.as_u16()));
        };
        let content_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        Ok(SourceResponse {
            kind,
            content_length,
            reader: Box::new(response),
        })
    }
}

pub(super) trait DownloadSource {
    fn open(&self, url: &Url, offset: u64) -> Result<SourceResponse, DownloadError>;
}

pub(super) struct SourceResponse {
    pub(super) kind: ResponseKind,
    pub(super) content_length: Option<u64>,
    pub(super) reader: Box<dyn Read>,
}

#[derive(Clone, Copy)]
pub(super) enum ResponseKind {
    Full,
    Partial { start: u64, total: u64 },
}

pub(super) fn parse_content_range(value: &str) -> Result<(u64, u64), DownloadError> {
    let range = value
        .strip_prefix("bytes ")
        .ok_or(DownloadError::InvalidContentRange)?;
    let (bounds, total) = range
        .split_once('/')
        .ok_or(DownloadError::InvalidContentRange)?;
    let (start, end) = bounds
        .split_once('-')
        .ok_or(DownloadError::InvalidContentRange)?;
    let start = start
        .parse::<u64>()
        .map_err(|_| DownloadError::InvalidContentRange)?;
    let end = end
        .parse::<u64>()
        .map_err(|_| DownloadError::InvalidContentRange)?;
    let total = total
        .parse::<u64>()
        .map_err(|_| DownloadError::InvalidContentRange)?;
    if start > end || end >= total {
        return Err(DownloadError::InvalidContentRange);
    }
    Ok((start, total))
}

pub(super) fn ensure_disk_space<F>(
    directory: &Path,
    artifact: &ModelArtifact,
    downloaded: u64,
    available_space: &F,
) -> Result<(), DownloadError>
where
    F: Fn(&Path) -> io::Result<u64>,
{
    let remaining_download = artifact.download_bytes().saturating_sub(downloaded);
    let required = remaining_download;
    let available = available_space(directory).map_err(DownloadError::Storage)?;
    if available < required {
        return Err(DownloadError::InsufficientDiskSpace {
            required,
            available,
        });
    }
    Ok(())
}

pub(super) fn verify_file(
    artifact: &ModelArtifact,
    path: &Path,
    cancellation: &DownloadCancellation,
) -> Result<VerifiedModelArtifact, DownloadError> {
    check_cancelled(cancellation)?;
    if !fs::symlink_metadata(path)
        .map_err(DownloadError::Storage)?
        .file_type()
        .is_file()
    {
        return Err(DownloadError::InstalledArtifactInvalid);
    }
    let mut file = File::open(path).map_err(DownloadError::Storage)?;
    let length = file.metadata().map_err(DownloadError::Storage)?.len();
    if length != artifact.download_bytes() {
        return Err(DownloadError::InstalledArtifactInvalid);
    }
    let actual = sha256_reader(&mut file, cancellation)?;
    if actual != artifact.sha256() {
        return Err(DownloadError::InstalledArtifactInvalid);
    }
    validate_format(artifact, &mut file)?;
    Ok(VerifiedModelArtifact {
        key: artifact.key().clone(),
        path: path.to_owned(),
        byte_size: length,
        sha256: actual,
    })
}

pub(super) fn sha256_reader(
    reader: &mut impl Read,
    cancellation: &DownloadCancellation,
) -> Result<String, DownloadError> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        check_cancelled(cancellation)?;
        let read = reader.read(&mut buffer).map_err(DownloadError::Storage)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(super) fn open_private_partial(path: &Path) -> Result<File, DownloadError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(DownloadError::Storage)?;
    #[cfg(unix)]
    fs::set_permissions(path, unix_permissions(0o600)).map_err(DownloadError::Storage)?;
    Ok(file)
}

pub(super) fn reset_partial(file: &mut File) -> Result<(), DownloadError> {
    file.set_len(0).map_err(DownloadError::Storage)?;
    file.seek(SeekFrom::Start(0))
        .map_err(DownloadError::Storage)?;
    file.sync_all().map_err(DownloadError::Storage)
}

pub(super) fn remove_partial_if_present(path: &Path) -> Result<(), DownloadError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DownloadError::Storage(error)),
    }
}

pub(super) fn create_private_directory(path: &Path) -> Result<(), DownloadError> {
    fs::create_dir_all(path).map_err(DownloadError::Storage)?;
    #[cfg(unix)]
    fs::set_permissions(path, unix_permissions(0o700)).map_err(DownloadError::Storage)?;
    Ok(())
}

pub(super) fn sync_directory(path: &Path) -> Result<(), DownloadError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(DownloadError::Storage)
}

#[cfg(unix)]
fn unix_permissions(mode: u32) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(mode)
}

pub(super) fn check_cancelled(cancellation: &DownloadCancellation) -> Result<(), DownloadError> {
    if cancellation.is_cancelled() {
        Err(DownloadError::Cancelled)
    } else {
        Ok(())
    }
}

// Digest verification authenticates the complete file. This additional bounded
// header check rejects a catalogue/runtime mismatch before publication or use.
pub(super) fn validate_format(
    artifact: &ModelArtifact,
    file: &mut File,
) -> Result<(), DownloadError> {
    if artifact.format() == crate::ArtifactFormat::WhisperGgml {
        crate::asr::artifact::validate_header(file)?;
    }
    Ok(())
}
