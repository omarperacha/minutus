use super::{
    AtFlags, DIFF_CONTEXT_LINES, Digest, File, FilesystemWriteChange, FilesystemWriteError,
    FmtWrite, MAX_WRITE_FILE_BYTES, MAX_WRITE_FILES, MAX_WRITE_TOTAL_BYTES, MetadataExt, Mode,
    OFlags, OsString, PreparedChange, Read, Sha256, WorkspaceDirectory, Write, WriteTarget, fs,
};

pub(super) fn validate_limits(
    changes: &[FilesystemWriteChange],
) -> Result<(), FilesystemWriteError> {
    if changes.is_empty() || changes.len() > MAX_WRITE_FILES {
        return Err(FilesystemWriteError::LimitExceeded);
    }
    let mut paths = std::collections::HashSet::with_capacity(changes.len());
    let mut total = 0_usize;
    for change in changes {
        if !paths.insert(change.relative_path.as_str())
            || change.content.len() > MAX_WRITE_FILE_BYTES
            || change.content.bytes().any(|byte| byte == 0)
        {
            return Err(FilesystemWriteError::LimitExceeded);
        }
        if let Some(digest) = &change.expected_sha256
            && (digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        {
            return Err(FilesystemWriteError::InvalidDigest);
        }
        total = total.saturating_add(change.content.len());
    }
    if total > MAX_WRITE_TOTAL_BYTES {
        return Err(FilesystemWriteError::LimitExceeded);
    }
    Ok(())
}

pub(super) fn revalidate(
    workspace: &WorkspaceDirectory,
    change: &PreparedChange,
) -> Result<WriteTarget, FilesystemWriteError> {
    let mut target = workspace.resolve_write_target(&change.relative_path)?;
    match (&mut target.existing, &change.expected_sha256) {
        (Some(file), Some(expected)) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(FilesystemWriteError::Io)?;
            if sha256(&bytes) != *expected
                || String::from_utf8(bytes).ok().as_deref() != Some(&change.previous_content)
            {
                return Err(FilesystemWriteError::Conflict);
            }
        }
        (None, None) => {}
        _ => return Err(FilesystemWriteError::Conflict),
    }
    Ok(target)
}

pub(super) fn stage_and_replace(
    workspace: &WorkspaceDirectory,
    change: &PreparedChange,
) -> Result<(), FilesystemWriteError> {
    let target = workspace.resolve_write_target(&change.relative_path)?;
    let temporary_name = OsString::from(format!(".zeta-write-{}", uuid::Uuid::now_v7()));
    let staged = fs::openat(
        &target.parent,
        &temporary_name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map(File::from)
    .map_err(io_error)?;
    let result = write_staged(&staged, change)
        .and_then(|()| revalidate(workspace, change))
        .and_then(|current_target| {
            let staged_parent = target.parent.metadata().map_err(FilesystemWriteError::Io)?;
            let current_parent = current_target
                .parent
                .metadata()
                .map_err(FilesystemWriteError::Io)?;
            if staged_parent.dev() != current_parent.dev()
                || staged_parent.ino() != current_parent.ino()
                || target.name != current_target.name
            {
                return Err(FilesystemWriteError::Conflict);
            }
            fs::renameat(
                &target.parent,
                &temporary_name,
                &target.parent,
                &target.name,
            )
            .map_err(io_error)
        })
        .and_then(|()| fs::fsync(&target.parent).map_err(io_error));
    if result.is_err() {
        let _ = fs::unlinkat(&target.parent, &temporary_name, AtFlags::empty());
    }
    result
}

pub(super) fn write_staged(
    staged: &File,
    change: &PreparedChange,
) -> Result<(), FilesystemWriteError> {
    let mode = u16::try_from(change.mode & 0o777).unwrap_or(0o600);
    fs::fchmod(staged, Mode::from_raw_mode(mode)).map_err(io_error)?;
    let mut staged = staged.try_clone().map_err(FilesystemWriteError::Io)?;
    staged
        .write_all(change.content.as_bytes())
        .map_err(FilesystemWriteError::Io)?;
    staged.flush().map_err(FilesystemWriteError::Io)?;
    fs::fsync(&staged).map_err(io_error)
}

pub(super) fn complete_diff(path: &str, previous: &str, next: &str, created: bool) -> String {
    let old_path = if created { "/dev/null" } else { path };
    let mut diff = format!("--- {old_path}\n+++ {path}\n");
    if previous == next {
        diff.push_str("@@ no textual changes @@\n");
        return diff;
    }

    let previous_lines = previous.split_inclusive('\n').collect::<Vec<_>>();
    let next_lines = next.split_inclusive('\n').collect::<Vec<_>>();
    let common_prefix = previous_lines
        .iter()
        .zip(&next_lines)
        .take_while(|(previous, next)| previous == next)
        .count();
    let maximum_suffix = previous_lines
        .len()
        .saturating_sub(common_prefix)
        .min(next_lines.len().saturating_sub(common_prefix));
    let common_suffix = previous_lines
        .iter()
        .rev()
        .zip(next_lines.iter().rev())
        .take(maximum_suffix)
        .take_while(|(previous, next)| previous == next)
        .count();
    let previous_change_end = previous_lines.len().saturating_sub(common_suffix);
    let next_change_end = next_lines.len().saturating_sub(common_suffix);
    let context_start = common_prefix.saturating_sub(DIFF_CONTEXT_LINES);
    let previous_context_end = previous_change_end
        .saturating_add(DIFF_CONTEXT_LINES)
        .min(previous_lines.len());
    let next_context_end = next_change_end
        .saturating_add(DIFF_CONTEXT_LINES)
        .min(next_lines.len());
    let previous_count = previous_context_end.saturating_sub(context_start);
    let next_count = next_context_end.saturating_sub(context_start);
    let previous_start = if previous_count == 0 {
        context_start
    } else {
        context_start.saturating_add(1)
    };
    let next_start = if next_count == 0 {
        context_start
    } else {
        context_start.saturating_add(1)
    };
    let _ = writeln!(
        diff,
        "@@ -{previous_start},{previous_count} +{next_start},{next_count} @@"
    );
    for line in &previous_lines[context_start..common_prefix] {
        push_diff_line(&mut diff, ' ', line);
    }
    for line in &previous_lines[common_prefix..previous_change_end] {
        push_diff_line(&mut diff, '-', line);
    }
    for line in &next_lines[common_prefix..next_change_end] {
        push_diff_line(&mut diff, '+', line);
    }
    for line in &next_lines[next_change_end..next_context_end] {
        push_diff_line(&mut diff, ' ', line);
    }
    diff
}

pub(super) fn push_diff_line(diff: &mut String, prefix: char, line: &str) {
    diff.push(prefix);
    diff.push_str(line);
    if !line.ends_with('\n') {
        diff.push('\n');
    }
}

pub(super) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn io_error(error: rustix::io::Errno) -> FilesystemWriteError {
    FilesystemWriteError::Io(error.into())
}
