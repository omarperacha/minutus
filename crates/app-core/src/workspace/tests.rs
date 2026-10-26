use std::{error::Error, fs};

use super::*;

#[test]
fn private_data_is_excluded_at_grant_and_existing_operation_admission() -> Result<(), Box<dyn Error>>
{
    let root = tempfile::tempdir()?;
    let private = root.path().join("private");
    let project = root.path().join("project");
    fs::create_dir(&project)?;
    let app = AppCore::initialize(&private)?;
    fs::create_dir(private.join("app-vault"))?;
    fs::write(
        private.join("app-vault/test.password-vault.json"),
        "synthetic",
    )?;

    for denied in [root.path(), &private, &private.join("app-vault")] {
        assert!(app.grant_workspace(denied, vec![1], "2026-10-22").is_err());
    }
    let grant = app.grant_workspace(&project, vec![1], "2026-10-22")?;
    app.update_workspace_permission(&grant.id, WorkspacePermissionLevel::ReadAndWrite)?;
    assert!(app.filesystem_list(&grant.id, &project, ".").is_ok());
    // Persisted grants/bookmarks from before this policy still cannot open private roots.
    assert!(
        app.filesystem_read_text(
            &grant.id,
            root.path(),
            "private/app-vault/test.password-vault.json"
        )
        .is_err()
    );
    assert!(app.filesystem_list(&grant.id, &private, ".").is_err());
    Ok(())
}
