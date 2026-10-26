//! Disposable fixture workspaces for the production-loop evaluator.

use std::{fs, path::Path};

use app_core::AppCore;
use ipc_types::{ProjectId, WorkspacePermissionLevel};

use super::EvalResult;

pub(super) fn install(
    core: &AppCore,
    project: &ProjectId,
    fixtures: &Path,
    relative: &str,
    destination: &Path,
) -> EvalResult<()> {
    let fixtures = fs::canonicalize(fixtures)?;
    let source = fs::canonicalize(fixtures.join(relative))?;
    if !source.starts_with(&fixtures) || source == fixtures || !source.is_dir() {
        return Err("workspace fixture must be a child directory of the suite directory".into());
    }
    copy_directory(&source, destination)?;
    let bookmark = crate::workspace_bookmark::create(destination)
        .map_err(|()| "cannot create isolated fixture bookmark")?;
    let grant =
        core.add_project_workspace(project, destination, bookmark, &super::trials::timestamp()?)?;
    core.update_project_workspace_permission(
        project,
        &grant.grant_id,
        WorkspacePermissionLevel::ReadAndWrite,
    )?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> EvalResult<()> {
    let parent = fs::canonicalize(destination.parent().ok_or("fixture target has no parent")?)?;
    if parent.starts_with(fs::canonicalize(source)?) {
        return Err("fixture target must be outside its source".into());
    }
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err("fixture symlinks and special files are not supported".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_trials_copy_inputs_without_mutating_the_source() -> EvalResult<()> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("source");
        fs::create_dir(&source)?;
        fs::write(source.join("input.txt"), "retained input")?;
        let target = temporary.path().join("trial");
        copy_directory(&source, &target)?;
        fs::write(target.join("input.txt"), "trial mutation")?;
        assert_eq!(
            fs::read_to_string(source.join("input.txt"))?,
            "retained input"
        );
        assert!(copy_directory(&source, &target).is_err());
        assert!(copy_directory(&source, &source.join("nested-trial")).is_err());
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn workspace_trials_reject_links_to_other_files() -> EvalResult<()> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("source");
        fs::create_dir(&source)?;
        fs::write(temporary.path().join("outside.txt"), "outside")?;
        std::os::unix::fs::symlink(temporary.path().join("outside.txt"), source.join("link"))?;
        assert!(copy_directory(&source, &temporary.path().join("trial")).is_err());
        Ok(())
    }
}
