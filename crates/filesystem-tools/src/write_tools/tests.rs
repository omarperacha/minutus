use std::{error::Error, fs, os::unix::fs::symlink};

use tempfile::tempdir;

use super::*;

#[test]
fn replaces_and_creates_text_files_atomically() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::create_dir(directory.path().join("src"))?;
    fs::write(directory.path().join("src/lib.rs"), "old\n")?;
    let workspace = WorkspaceDirectory::open(directory.path())?;
    let result = write_text(
        &workspace,
        &[
            FilesystemWriteChange {
                relative_path: "src/lib.rs".to_owned(),
                expected_sha256: Some(sha256(b"old\n")),
                content: "new\n".to_owned(),
                create_missing_parents: false,
            },
            FilesystemWriteChange {
                relative_path: "src/new.rs".to_owned(),
                expected_sha256: None,
                content: "created\n".to_owned(),
                create_missing_parents: false,
            },
        ],
    )?;

    assert_eq!(
        fs::read_to_string(directory.path().join("src/lib.rs"))?,
        "new\n"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("src/new.rs"))?,
        "created\n"
    );
    assert_eq!(result.changes.len(), 2);
    assert!(!result.changes[0].created);
    assert!(result.changes[0].post_write_verified);
    assert_eq!(result.changes[0].verified_byte_size, 4);
    assert!(result.changes[0].diff.contains("-old\n+new\n"));
    assert!(result.changes[1].created);
    assert!(result.changes[1].post_write_verified);
    assert_eq!(result.changes[1].verified_byte_size, 8);
    assert!(result.changes[1].diff.starts_with("--- /dev/null"));
    Ok(())
}

#[test]
fn creates_only_target_parents_and_undo_removes_them_when_empty() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::create_dir(directory.path().join("existing"))?;
    let workspace = WorkspaceDirectory::open(directory.path())?;
    let result = write_text(
        &workspace,
        &[FilesystemWriteChange {
            relative_path: "existing/new/deep/report.txt".to_owned(),
            expected_sha256: None,
            content: "verified\n".to_owned(),
            create_missing_parents: true,
        }],
    )?;
    assert!(result.postcondition_verified);
    assert_eq!(
        result.changes[0].created_parent_directories,
        ["existing/new", "existing/new/deep"]
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("existing/new/deep/report.txt"))?,
        "verified\n"
    );

    let change = &result.changes[0];
    let undone = undo_text(
        &workspace,
        &[FilesystemUndoChange {
            relative_path: change.relative_path.clone(),
            applied_sha256: change.new_sha256.clone(),
            previous_content: None,
            created_parent_directories: change.created_parent_directories.clone(),
        }],
    );
    assert_eq!(undone.files[0].status, FilesystemUndoStatus::Undone);
    assert!(directory.path().join("existing").is_dir());
    assert!(!directory.path().join("existing/new").exists());
    Ok(())
}

#[test]
fn parent_aware_create_rejects_conflicts_and_rolls_back_failed_verification()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::write(directory.path().join("conflict.txt"), "existing")?;
    let workspace = WorkspaceDirectory::open(directory.path())?;
    let conflict = write_text(
        &workspace,
        &[FilesystemWriteChange {
            relative_path: "conflict.txt".to_owned(),
            expected_sha256: None,
            content: "replacement".to_owned(),
            create_missing_parents: true,
        }],
    );
    assert!(matches!(conflict, Err(FilesystemWriteError::Conflict)));
    assert_eq!(
        fs::read_to_string(directory.path().join("conflict.txt"))?,
        "existing"
    );

    let failed = write_text_with_verifier(
        &workspace,
        &[FilesystemWriteChange {
            relative_path: "new/deep/report.txt".to_owned(),
            expected_sha256: None,
            content: "temporary".to_owned(),
            create_missing_parents: true,
        }],
        |_, _| false,
    );
    assert!(matches!(
        failed,
        Err(FilesystemWriteError::PostconditionFailed)
    ));
    assert!(!directory.path().join("new").exists());
    Ok(())
}

#[test]
fn review_diff_keeps_only_changed_lines_and_nearby_context() {
    let mut previous = String::new();
    for line in 1..=700 {
        let _ = writeln!(previous, "line {line}");
    }
    let next = previous.replace("line 420\n", "ZETA Dev | Real G\n");
    let diff = complete_diff("cv.html", &previous, &next, false);

    assert!(diff.contains("-line 420\n+ZETA Dev | Real G\n"));
    assert!(diff.contains(" line 417\n"));
    assert!(diff.contains(" line 423\n"));
    assert!(!diff.contains(" line 1\n"));
    assert!(diff.len() < 512);
}

#[test]
fn stale_batch_is_rejected_before_any_write() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::write(directory.path().join("first.txt"), "first\n")?;
    fs::write(directory.path().join("second.txt"), "second\n")?;
    let workspace = WorkspaceDirectory::open(directory.path())?;
    let result = write_text(
        &workspace,
        &[
            FilesystemWriteChange {
                relative_path: "first.txt".to_owned(),
                expected_sha256: Some(sha256(b"first\n")),
                content: "changed\n".to_owned(),
                create_missing_parents: false,
            },
            FilesystemWriteChange {
                relative_path: "second.txt".to_owned(),
                expected_sha256: Some(sha256(b"stale\n")),
                content: "changed\n".to_owned(),
                create_missing_parents: false,
            },
        ],
    );
    assert!(matches!(result, Err(FilesystemWriteError::Conflict)));
    assert_eq!(
        fs::read_to_string(directory.path().join("first.txt"))?,
        "first\n"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("second.txt"))?,
        "second\n"
    );
    Ok(())
}

#[test]
fn undo_restores_replacements_removes_creations_and_reports_conflicts() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    fs::write(directory.path().join("existing.txt"), "before\n")?;
    let workspace = WorkspaceDirectory::open(directory.path())?;
    let applied = write_text(
        &workspace,
        &[
            FilesystemWriteChange {
                relative_path: "existing.txt".to_owned(),
                expected_sha256: Some(sha256(b"before\n")),
                content: "after\n".to_owned(),
                create_missing_parents: false,
            },
            FilesystemWriteChange {
                relative_path: "created.txt".to_owned(),
                expected_sha256: None,
                content: "created\n".to_owned(),
                create_missing_parents: false,
            },
        ],
    )?;
    let changes = applied
        .changes
        .into_iter()
        .map(|change| FilesystemUndoChange {
            relative_path: change.relative_path,
            applied_sha256: change.new_sha256,
            previous_content: change.previous_content,
            created_parent_directories: change.created_parent_directories,
        })
        .collect::<Vec<_>>();
    assert!(undo_available(&workspace, &changes));
    let undone = undo_text(&workspace, &changes);
    assert!(
        undone
            .files
            .iter()
            .all(|file| file.status == FilesystemUndoStatus::Undone)
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("existing.txt"))?,
        "before\n"
    );
    assert!(!directory.path().join("created.txt").exists());
    assert!(!undo_available(&workspace, &changes));

    fs::write(directory.path().join("existing.txt"), "external\n")?;
    let conflict = undo_text(&workspace, &changes[..1]);
    assert_eq!(conflict.files[0].status, FilesystemUndoStatus::Conflict);
    assert_eq!(
        fs::read_to_string(directory.path().join("existing.txt"))?,
        "external\n"
    );
    Ok(())
}

#[test]
fn targeted_replacement_requires_digest_and_exact_occurrence_count() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::write(
        directory.path().join("cv.html"),
        "<h1>Name</h1>\n<p>Old subheader</p>\n",
    )?;
    let workspace = WorkspaceDirectory::open(directory.path())?;
    let digest = sha256(b"<h1>Name</h1>\n<p>Old subheader</p>\n");
    let result = replace_text(
        &workspace,
        "cv.html",
        &digest,
        &[FilesystemTextReplacement {
            old_text: "Old subheader".to_owned(),
            new_text: "ZETA Dev | Real G".to_owned(),
            expected_occurrences: 1,
        }],
    )?;
    assert_eq!(result.changes.len(), 1);
    assert_eq!(
        fs::read_to_string(directory.path().join("cv.html"))?,
        "<h1>Name</h1>\n<p>ZETA Dev | Real G</p>\n"
    );

    let conflict = replace_text(
        &workspace,
        "cv.html",
        &result.changes[0].new_sha256,
        &[FilesystemTextReplacement {
            old_text: "missing".to_owned(),
            new_text: "replacement".to_owned(),
            expected_occurrences: 1,
        }],
    );
    assert!(matches!(conflict, Err(FilesystemWriteError::Conflict)));
    assert_eq!(
        fs::read_to_string(directory.path().join("cv.html"))?,
        "<h1>Name</h1>\n<p>ZETA Dev | Real G</p>\n"
    );
    Ok(())
}

#[test]
fn unsafe_targets_and_limits_fail_closed() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let outside = tempdir()?;
    fs::write(outside.path().join("secret.txt"), "secret\n")?;
    symlink(
        outside.path().join("secret.txt"),
        directory.path().join("link.txt"),
    )?;
    let workspace = WorkspaceDirectory::open(directory.path())?;
    assert!(matches!(
        write_text(
            &workspace,
            &[FilesystemWriteChange {
                relative_path: "link.txt".to_owned(),
                expected_sha256: None,
                content: "overwrite\n".to_owned(),
                create_missing_parents: false,
            }]
        ),
        Err(FilesystemWriteError::Path(SafePathError::SymbolicLink))
    ));
    assert!(matches!(
        write_text(
            &workspace,
            &[FilesystemWriteChange {
                relative_path: "large.txt".to_owned(),
                expected_sha256: None,
                content: "x".repeat(MAX_WRITE_FILE_BYTES + 1),
                create_missing_parents: false,
            }]
        ),
        Err(FilesystemWriteError::LimitExceeded)
    ));
    assert_eq!(
        fs::read_to_string(outside.path().join("secret.txt"))?,
        "secret\n"
    );
    Ok(())
}
