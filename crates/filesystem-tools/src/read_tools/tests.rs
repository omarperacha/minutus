use std::{error::Error, fs, os::unix::fs::symlink};

use tempfile::tempdir;

use super::*;

#[test]
fn lists_stats_reads_and_searches_safe_text_entries() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::create_dir(directory.path().join("src"))?;
    fs::write(
        directory.path().join("src/lib.rs"),
        "first needle\nsecond needle\n",
    )?;
    fs::write(directory.path().join("README.md"), "no match\n")?;
    let workspace = WorkspaceDirectory::open(directory.path())?;

    let listing = list(&workspace, ".")?;
    assert_eq!(listing.entries.len(), 2);
    assert_eq!(listing.entries[0].name, "README.md");
    assert_eq!(listing.entries[1].name, "src");
    assert_eq!(listing.omitted_count, 0);
    assert!(!listing.truncated);

    let metadata = stat(&workspace, "src/lib.rs")?;
    assert_eq!(metadata.kind, FilesystemEntryKind::File);
    assert_eq!(metadata.byte_size, Some(27));

    let read = read_text(&workspace, "src/lib.rs")?;
    assert_eq!(read.text, "first needle\nsecond needle\n");
    assert_eq!(read.byte_size, 27);

    let result = search(&workspace, ".", "needle")?;
    assert_eq!(result.files_searched, 2);
    assert_eq!(result.matches.len(), 2);
    assert_eq!(result.matches[0].relative_path, "src/lib.rs");
    assert_eq!(result.matches[0].line_number, 1);
    assert_eq!(result.matches[0].column, 7);
    assert!(!result.truncated);
    Ok(())
}

#[test]
fn read_text_rejects_oversized_invalid_and_binary_files() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::write(
        directory.path().join("large.txt"),
        vec![b'x'; MAX_READ_TEXT_BYTES + 1],
    )?;
    fs::write(directory.path().join("invalid.txt"), [0xff, 0xfe])?;
    fs::write(directory.path().join("binary.txt"), b"text\0bytes")?;
    let workspace = WorkspaceDirectory::open(directory.path())?;

    assert!(matches!(
        read_text(&workspace, "large.txt"),
        Err(FilesystemToolError::FileTooLarge)
    ));
    assert!(matches!(
        read_text(&workspace, "invalid.txt"),
        Err(FilesystemToolError::InvalidUtf8)
    ));
    assert!(matches!(
        read_text(&workspace, "binary.txt"),
        Err(FilesystemToolError::BinaryFile)
    ));
    Ok(())
}

#[test]
fn reads_bounded_binary_bytes_without_following_links() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let outside = tempdir()?;
    fs::write(directory.path().join("image.bin"), [0_u8, 1, 2, 3])?;
    fs::write(outside.path().join("outside.bin"), [4_u8, 5, 6])?;
    symlink(
        outside.path().join("outside.bin"),
        directory.path().join("linked.bin"),
    )?;
    let workspace = WorkspaceDirectory::open(directory.path())?;

    let read = read_bytes(&workspace, "image.bin", 4)?;
    assert_eq!(read.relative_path, "image.bin");
    assert_eq!(read.bytes, [0, 1, 2, 3]);
    assert_eq!(read.byte_size, 4);
    assert_eq!(
        read.content_sha256,
        "054edec1d0211f624fed0cbca9d4f9400b0e491c43742af2c5b0abebf0c990d8"
    );
    assert!(matches!(
        read_bytes(&workspace, "image.bin", 3),
        Err(FilesystemToolError::FileTooLarge)
    ));
    assert!(matches!(
        read_bytes(&workspace, "linked.bin", 4),
        Err(FilesystemToolError::Path(_))
    ));
    Ok(())
}

#[test]
fn listing_and_search_omit_symlinks_and_binary_files() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let outside = tempdir()?;
    fs::write(directory.path().join("safe.txt"), "needle\n")?;
    fs::write(directory.path().join("binary.dat"), b"needle\0")?;
    fs::write(outside.path().join("secret.txt"), "needle outside\n")?;
    symlink(
        outside.path().join("secret.txt"),
        directory.path().join("outside-link"),
    )?;
    let workspace = WorkspaceDirectory::open(directory.path())?;

    let listing = list(&workspace, ".")?;
    assert_eq!(listing.entries.len(), 2);
    assert_eq!(listing.omitted_count, 1);
    let result = search(&workspace, ".", "needle")?;
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].relative_path, "safe.txt");
    assert_eq!(result.omitted_count, 2);
    Ok(())
}

#[test]
fn directory_listing_and_search_have_hard_result_limits() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    for index in 0..=MAX_LIST_ENTRIES {
        fs::write(directory.path().join(format!("file-{index:04}.txt")), b"x")?;
    }
    let workspace = WorkspaceDirectory::open(directory.path())?;

    let listing = list(&workspace, ".")?;
    assert_eq!(listing.entries.len(), MAX_LIST_ENTRIES);
    assert!(listing.truncated);

    let match_directory = tempdir()?;
    let match_file = match_directory.path().join("matches.txt");
    fs::write(&match_file, "needle\n".repeat(MAX_SEARCH_MATCHES + 1))?;
    let match_workspace = WorkspaceDirectory::open(match_directory.path())?;
    let result = search(&match_workspace, ".", "needle")?;
    assert_eq!(result.matches.len(), MAX_SEARCH_MATCHES);
    assert!(result.truncated);
    Ok(())
}

#[test]
fn invalid_search_queries_fail_before_traversal() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let workspace = WorkspaceDirectory::open(directory.path())?;
    assert!(matches!(
        search(&workspace, ".", ""),
        Err(FilesystemToolError::InvalidQuery)
    ));
    assert!(matches!(
        search(&workspace, ".", &"x".repeat(MAX_SEARCH_QUERY_BYTES + 1)),
        Err(FilesystemToolError::InvalidQuery)
    ));
    Ok(())
}

#[test]
fn scan_counts_lines_without_returning_contents() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    fs::create_dir(directory.path().join("src"))?;
    fs::create_dir(directory.path().join("target"))?;
    fs::write(directory.path().join("short.rs"), "one\ntwo\n")?;
    fs::write(directory.path().join("README.md"), "one\ntwo\nthree\n")?;
    fs::write(directory.path().join("src/large.rs"), "one\ntwo\nthree")?;
    fs::write(
        directory.path().join("target/generated.rs"),
        "one\ntwo\nthree\nfour\n",
    )?;
    fs::write(directory.path().join("binary.dat"), b"one\0two\n")?;
    let workspace = WorkspaceDirectory::open(directory.path())?;

    let result = scan(&workspace, ".", true, 3, &["md".to_owned()])?;
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].relative_path, "src/large.rs");
    assert_eq!(result.matches[0].line_count, 3);
    assert_eq!(result.files_scanned, 2);
    assert_eq!(result.directories_scanned, 2);
    assert_eq!(result.omitted_count, 1);
    assert_eq!(result.excluded_count, 2);
    assert!(!result.truncated);
    Ok(())
}

#[test]
fn scan_validates_utf8_across_read_boundaries() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let mut text = "x".repeat(64 * 1024 - 1);
    text.push('ζ');
    text.push_str("\nsecond\n");
    fs::write(directory.path().join("unicode.txt"), text)?;
    let workspace = WorkspaceDirectory::open(directory.path())?;

    let result = scan(&workspace, ".", false, 2, &[])?;
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_count, 2);
    assert_eq!(result.omitted_count, 0);
    Ok(())
}

#[test]
fn scan_rejects_malformed_extension_filters() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let workspace = WorkspaceDirectory::open(directory.path())?;
    assert!(matches!(
        scan(&workspace, ".", true, 0, &[".md".to_owned()]),
        Err(FilesystemToolError::InvalidScanFilter)
    ));
    Ok(())
}
