//! Protected paths remain excluded across discovery, content, and mutation tools.

use std::{error::Error, fs};

use filesystem_tools::{
    FilesystemToolError, FilesystemWriteChange, FilesystemWriteError, SafePathError,
    WorkspaceDirectory, create_directory, list, read_bytes, read_text, scan, search, stat,
    write_text,
};

#[test]
fn credentials_are_never_returned_by_exact_or_recursive_reads() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join(".env"), "SYNTHETIC_SECRET=needle")?;
    fs::write(root.path().join("notes.txt"), "needle ordinary")?;
    let workspace = WorkspaceDirectory::open(root.path())?;
    assert!(matches!(
        read_text(&workspace, ".env"),
        Err(FilesystemToolError::Path(SafePathError::ProtectedPath))
    ));
    assert!(read_bytes(&workspace, ".env", 1024).is_err());
    assert!(stat(&workspace, ".env").is_err());
    let listing = list(&workspace, ".")?;
    assert!(listing.entries.iter().all(|entry| entry.name != ".env"));
    let found = search(&workspace, ".", "needle")?;
    assert_eq!(found.matches.len(), 1);
    assert_eq!(found.matches[0].relative_path, "notes.txt");
    let scanned = scan(&workspace, ".", true, 0, &[])?;
    assert!(
        scanned
            .matches
            .iter()
            .all(|entry| entry.relative_path != ".env")
    );
    Ok(())
}

#[test]
fn write_grants_cannot_modify_execution_controls_or_create_their_parents()
-> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join(".git"))?;
    let workspace = WorkspaceDirectory::open(root.path())?;
    for path in [
        ".git/config",
        ".vscode/tasks.json",
        ".github/workflows/build.yml",
        ".zshrc",
    ] {
        let result = write_text(
            &workspace,
            &[FilesystemWriteChange {
                relative_path: path.into(),
                expected_sha256: None,
                content: "synthetic marker".into(),
                create_missing_parents: true,
            }],
        );
        assert!(
            matches!(
                result,
                Err(FilesystemWriteError::Path(SafePathError::ProtectedPath))
            ),
            "{path}"
        );
        assert!(!root.path().join(path).exists());
    }
    assert!(!root.path().join(".vscode").exists());
    assert!(create_directory(&workspace, ".ssh").is_err());
    fs::create_dir(root.path().join("src"))?;
    let written = write_text(
        &workspace,
        &[FilesystemWriteChange {
            relative_path: "src/main.rs".into(),
            expected_sha256: None,
            content: "fn main() {}".into(),
            create_missing_parents: false,
        }],
    )?;
    assert!(written.postcondition_verified);
    Ok(())
}

#[test]
fn selecting_a_protected_directory_or_child_as_root_does_not_bypass_policy()
-> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    for path in [".git", ".vscode", ".ssh/nested", ".github/workflows"] {
        let path = root.path().join(path);
        fs::create_dir_all(&path)?;
        assert!(matches!(
            WorkspaceDirectory::open(&path),
            Err(SafePathError::ProtectedPath)
        ));
    }
    Ok(())
}
