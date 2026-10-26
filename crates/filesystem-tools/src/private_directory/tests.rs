use std::{error::Error, fs};

use tempfile::tempdir;

use super::*;
use crate::{WorkspaceDirectory, read_text};

#[test]
fn excludes_private_roots_descendants_and_broad_grants() -> Result<(), Box<dyn Error>> {
    let root = tempdir()?;
    let private = root.path().join("private");
    fs::create_dir_all(private.join("app-vault"))?;
    let ordinary = root.path().join("project");
    fs::create_dir(&ordinary)?;
    let policy = PrivateDirectory::open(&private)?;
    for denied in [root.path(), &private, &private.join("app-vault")] {
        assert!(matches!(
            WorkspaceDirectory::open_excluding(denied, &policy),
            Err(SafePathError::ProtectedPath)
        ));
    }
    fs::write(ordinary.join("source.txt"), "ordinary")?;
    let workspace = WorkspaceDirectory::open_excluding(&ordinary, &policy)?;
    assert!(read_text(&workspace, "source.txt").is_ok());
    Ok(())
}

#[test]
fn retained_private_identity_survives_rename_and_replacement() -> Result<(), Box<dyn Error>> {
    let root = tempdir()?;
    let private = root.path().join("private");
    fs::create_dir(&private)?;
    let policy = PrivateDirectory::open(&private)?;
    let moved = root.path().join("renamed");
    fs::rename(&private, &moved)?;
    fs::create_dir(&private)?;
    assert!(matches!(
        WorkspaceDirectory::open_excluding(&moved, &policy),
        Err(SafePathError::ProtectedPath)
    ));
    assert!(matches!(
        WorkspaceDirectory::open_excluding(&private, &policy),
        Err(SafePathError::ProtectedPath)
    ));
    Ok(())
}

#[test]
fn existing_capability_rejects_private_directory_moved_under_it() -> Result<(), Box<dyn Error>> {
    let root = tempdir()?;
    let private = root.path().join("private");
    let project = root.path().join("project");
    fs::create_dir(&private)?;
    fs::create_dir(&project)?;
    fs::write(private.join("vault.json"), "encrypted synthetic vault")?;
    let policy = PrivateDirectory::open(&private)?;
    let workspace = WorkspaceDirectory::open_excluding(&project, &policy)?;
    fs::rename(&private, project.join("moved"))?;
    assert!(workspace.open_directory(".").is_err());
    assert!(workspace.open_file("moved/vault.json").is_err());
    assert!(workspace.resolve_write_target("moved/vault.json").is_err());
    assert!(workspace.resolve_new_directory_target("moved/new").is_err());
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn sandbox_checks_ancestry_without_directory_listing_access() -> Result<(), Box<dyn Error>> {
    const CHILD: &str = "MINUTUS_PRIVATE_BOUNDARY_SANDBOX_FIXTURE";
    if let Some(path) = std::env::var_os(CHILD) {
        let ancestor = std::path::PathBuf::from(path);
        assert!(fs::read_dir(&ancestor).is_err(), "sandbox negative control");
        let private = PrivateDirectory::open(ancestor.join("private"))?;
        let workspace = WorkspaceDirectory::open_excluding(ancestor.join("project"), &private)?;
        assert!(workspace.open_file("ordinary.txt").is_ok());
        assert!(WorkspaceDirectory::open_excluding(ancestor.join("private"), &private).is_err());
        return Ok(());
    }
    let root = tempdir()?;
    let ancestor = root.path().canonicalize()?;
    fs::create_dir(ancestor.join("private"))?;
    fs::create_dir(ancestor.join("project"))?;
    fs::write(ancestor.join("project/ordinary.txt"), "ordinary")?;
    // tempfile generates no profile syntax; use an explicit checked literal.
    let literal = ancestor.to_str().ok_or("non-UTF8 fixture")?;
    assert!(!literal.contains(['"', '\\']));
    let profile =
        format!("(version 1)(allow default)(deny file-read-data (literal \"{literal}\"))");
    let output = std::process::Command::new("/usr/bin/sandbox-exec")
        .args(["-p", &profile])
        .arg(std::env::current_exe()?)
        .args([
            "--exact",
            "private_directory::tests::sandbox_checks_ancestry_without_directory_listing_access",
            "--nocapture",
        ])
        .env(CHILD, &ancestor)
        .output()?;
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
