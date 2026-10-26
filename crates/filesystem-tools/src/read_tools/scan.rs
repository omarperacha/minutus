use super::{
    DEFAULT_SCAN_EXCLUDED_DIRECTORIES, Digest, File, FilesystemEntryKind, FilesystemReadTextResult,
    FilesystemScanMatch, FilesystemScanResult, FilesystemSearchMatch, FilesystemToolError, Instant,
    MAX_MATCH_LINE_BYTES, MAX_SCAN_DEPTH, MAX_SCAN_DIRECTORIES, MAX_SCAN_DURATION,
    MAX_SCAN_FILE_BYTES, MAX_SCAN_FILES, MAX_SCAN_MATCHES, MAX_SCAN_TOTAL_BYTES,
    MAX_SEARCH_MATCHES, MetadataExt, OsStr, Path, PathBuf, Read, SafePathError, Sha256,
    WorkspaceDirectory, list,
};

/// Scans supported text files and returns bounded line-count metadata.
///
/// # Errors
///
/// Returns an error when the scope is invalid, a file changes during the read,
/// a file is unsafe to inspect, or an I/O operation fails.
#[allow(clippy::too_many_lines)]
pub fn scan(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
    recursive: bool,
    minimum_line_count: usize,
    excluded_extensions: &[String],
) -> Result<FilesystemScanResult, FilesystemToolError> {
    if excluded_extensions.len() > 32
        || excluded_extensions.iter().any(|extension| {
            extension.is_empty()
                || extension.len() > 16
                || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        return Err(FilesystemToolError::InvalidScanFilter);
    }
    let excluded_extensions = excluded_extensions
        .iter()
        .map(|extension| extension.to_ascii_lowercase())
        .collect::<Vec<_>>();
    workspace.open_directory(relative_path)?;
    let started = Instant::now();
    let mut pending = vec![(relative_path.to_owned(), 0_usize)];
    let mut matches = Vec::new();
    let mut files_scanned = 0_usize;
    let mut files_considered = 0_usize;
    let mut directories_scanned = 0_usize;
    let mut bytes_scanned = 0_usize;
    let mut omitted_count = 0_usize;
    let mut excluded_count = 0_usize;
    let mut truncated = false;

    'scan: while let Some((directory, depth)) = pending.pop() {
        if directories_scanned >= MAX_SCAN_DIRECTORIES || started.elapsed() >= MAX_SCAN_DURATION {
            truncated = true;
            break;
        }
        directories_scanned += 1;
        let listing = list(workspace, &directory)?;
        omitted_count = omitted_count.saturating_add(listing.omitted_count);
        truncated |= listing.truncated;
        for entry in listing.entries.into_iter().rev() {
            if started.elapsed() >= MAX_SCAN_DURATION {
                truncated = true;
                break 'scan;
            }
            match entry.kind {
                FilesystemEntryKind::Directory => {
                    if DEFAULT_SCAN_EXCLUDED_DIRECTORIES.contains(&entry.name.as_str()) {
                        excluded_count += 1;
                    } else if recursive {
                        if depth >= MAX_SCAN_DEPTH || pending.len() >= MAX_SCAN_DIRECTORIES {
                            truncated = true;
                        } else {
                            pending.push((entry.relative_path, depth + 1));
                        }
                    } else {
                        excluded_count += 1;
                    }
                }
                FilesystemEntryKind::File => {
                    if excluded_extension(&entry.relative_path, &excluded_extensions) {
                        excluded_count += 1;
                        continue;
                    }
                    if files_considered >= MAX_SCAN_FILES || bytes_scanned >= MAX_SCAN_TOTAL_BYTES {
                        truncated = true;
                        break 'scan;
                    }
                    files_considered += 1;
                    let remaining = MAX_SCAN_TOTAL_BYTES - bytes_scanned;
                    let limit = MAX_SCAN_FILE_BYTES.min(remaining);
                    if entry.byte_size.is_some_and(|byte_size| {
                        byte_size > u64::try_from(limit).unwrap_or(u64::MAX)
                    }) {
                        omitted_count += 1;
                        truncated |= limit < MAX_SCAN_FILE_BYTES;
                        continue;
                    }
                    match count_text_lines(workspace, &entry.relative_path, limit) {
                        Ok((line_count, byte_size)) => {
                            files_scanned += 1;
                            bytes_scanned = bytes_scanned
                                .saturating_add(usize::try_from(byte_size).unwrap_or(limit));
                            if line_count >= minimum_line_count {
                                if matches.len() >= MAX_SCAN_MATCHES {
                                    truncated = true;
                                    break 'scan;
                                }
                                matches.push(FilesystemScanMatch {
                                    relative_path: entry.relative_path,
                                    byte_size,
                                    line_count,
                                });
                            }
                        }
                        Err(
                            FilesystemToolError::FileTooLarge
                            | FilesystemToolError::InvalidUtf8
                            | FilesystemToolError::BinaryFile
                            | FilesystemToolError::ChangedDuringRead
                            | FilesystemToolError::Path(_),
                        ) => omitted_count += 1,
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }
    matches.sort_by(|left, right| {
        left.relative_path
            .as_bytes()
            .cmp(right.relative_path.as_bytes())
    });
    Ok(FilesystemScanResult {
        matches,
        files_scanned,
        directories_scanned,
        omitted_count,
        excluded_count,
        truncated,
    })
}

fn excluded_extension(relative_path: &str, excluded_extensions: &[String]) -> bool {
    Path::new(relative_path)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            excluded_extensions
                .iter()
                .any(|excluded| extension.eq_ignore_ascii_case(excluded))
        })
}

fn count_text_lines(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
    limit: usize,
) -> Result<(usize, u64), FilesystemToolError> {
    let mut file = workspace.open_file(relative_path)?;
    let before = file.metadata().map_err(FilesystemToolError::Io)?;
    if before.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(FilesystemToolError::FileTooLarge);
    }
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    let mut utf8_tail = Vec::with_capacity(4);
    let mut total = 0_usize;
    let mut line_count = 0_usize;
    let mut final_byte = None;
    loop {
        let read = file.read(&mut buffer).map_err(FilesystemToolError::Io)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        if total > limit {
            return Err(FilesystemToolError::FileTooLarge);
        }
        let mut bytes = Vec::with_capacity(utf8_tail.len() + read);
        bytes.append(&mut utf8_tail);
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.contains(&0) {
            return Err(FilesystemToolError::BinaryFile);
        }
        let valid_bytes = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.as_slice(),
            Err(error) if error.error_len().is_none() => {
                let valid_up_to = error.valid_up_to();
                utf8_tail.extend_from_slice(&bytes[valid_up_to..]);
                &bytes[..valid_up_to]
            }
            Err(_) => return Err(FilesystemToolError::InvalidUtf8),
        };
        line_count = line_count.saturating_add(count_newlines(valid_bytes));
        final_byte = valid_bytes.last().copied().or(final_byte);
    }
    if !utf8_tail.is_empty() {
        std::str::from_utf8(&utf8_tail).map_err(|_| FilesystemToolError::InvalidUtf8)?;
        line_count = line_count.saturating_add(count_newlines(&utf8_tail));
        final_byte = utf8_tail.last().copied().or(final_byte);
    }
    if total > 0 && final_byte != Some(b'\n') {
        line_count = line_count.saturating_add(1);
    }
    let after = file.metadata().map_err(FilesystemToolError::Io)?;
    if file_changed(&before, &after) {
        return Err(FilesystemToolError::ChangedDuringRead);
    }
    Ok((line_count, before.len()))
}

fn count_newlines(bytes: &[u8]) -> usize {
    let mut count = 0_usize;
    for byte in bytes {
        if *byte == b'\n' {
            count = count.saturating_add(1);
        }
    }
    count
}

pub(super) fn open_supported(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
) -> Result<(File, FilesystemEntryKind), FilesystemToolError> {
    match workspace.open_file(relative_path) {
        Ok(file) => Ok((file, FilesystemEntryKind::File)),
        Err(SafePathError::UnexpectedFileType) => workspace
            .open_directory(relative_path)
            .map(|directory| (directory, FilesystemEntryKind::Directory))
            .map_err(Into::into),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn read_text_with_limit(
    workspace: &WorkspaceDirectory,
    relative_path: &str,
    limit: usize,
) -> Result<FilesystemReadTextResult, FilesystemToolError> {
    let mut file = workspace.open_file(relative_path)?;
    let before = file.metadata().map_err(FilesystemToolError::Io)?;
    if before.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(FilesystemToolError::FileTooLarge);
    }
    let capacity = usize::try_from(before.len()).unwrap_or(limit).min(limit);
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(FilesystemToolError::Io)?;
    if bytes.len() > limit {
        return Err(FilesystemToolError::FileTooLarge);
    }
    let after = file.metadata().map_err(FilesystemToolError::Io)?;
    if file_changed(&before, &after) {
        return Err(FilesystemToolError::ChangedDuringRead);
    }
    let text = String::from_utf8(bytes).map_err(|_| FilesystemToolError::InvalidUtf8)?;
    if text.bytes().any(|byte| byte == 0) {
        return Err(FilesystemToolError::BinaryFile);
    }
    Ok(FilesystemReadTextResult {
        relative_path: relative_path.to_owned(),
        byte_size: u64::try_from(text.len()).unwrap_or(u64::MAX),
        content_sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
        text,
    })
}

pub(super) fn file_changed(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
}

pub(super) fn child_path(parent: &str, name: &str) -> String {
    if parent == "." {
        name.to_owned()
    } else {
        let mut path = PathBuf::from(parent);
        path.push(OsStr::new(name));
        path.to_string_lossy().into_owned()
    }
}

pub(super) fn collect_matches(
    matches: &mut Vec<FilesystemSearchMatch>,
    relative_path: &str,
    text: &str,
    query: &str,
) {
    for (line_index, line) in text.lines().enumerate() {
        for (column, _) in line.match_indices(query) {
            if matches.len() >= MAX_SEARCH_MATCHES {
                return;
            }
            matches.push(FilesystemSearchMatch {
                relative_path: relative_path.to_owned(),
                line_number: line_index + 1,
                column: column + 1,
                line: bounded_line(line),
            });
        }
    }
}

fn bounded_line(line: &str) -> String {
    if line.len() <= MAX_MATCH_LINE_BYTES {
        return line.to_owned();
    }
    let mut end = MAX_MATCH_LINE_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

pub(super) fn io_error(error: rustix::io::Errno) -> FilesystemToolError {
    FilesystemToolError::Io(error.into())
}
