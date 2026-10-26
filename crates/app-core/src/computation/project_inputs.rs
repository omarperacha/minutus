//! Host-only binding of granted project files to pathless computation inputs.

use std::{collections::BTreeSet, error::Error, fmt, path::Path};

use app_storage::WorkspaceGrant;
use computation_runtime::{
    ComputationInput, ContractError, InputFormat, MAX_INPUT_BYTES, MAX_INPUTS,
    MAX_TOTAL_INPUT_BYTES,
};
use filesystem_tools::{FilesystemToolError, WorkspaceOperation};
use ipc_types::{ProjectId, WorkspaceGrantId, WorkspacePermissionLevel};
use sha2::{Digest, Sha256};

use crate::{AppCore, AppCoreError};

const MAX_RELATIVE_PATH_BYTES: usize = 4_096;

/// One validated host-side project file reference awaiting safe resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectComputationInputReference {
    id: String,
    relative_path: String,
    format: InputFormat,
}

impl ProjectComputationInputReference {
    /// Creates one bounded reference. Filesystem safety is still decided by the
    /// handle-relative broker when the reference is bound.
    ///
    /// # Errors
    ///
    /// Returns an error when the request-local ID or path size is invalid.
    pub fn new(
        id: impl Into<String>,
        relative_path: impl Into<String>,
        format: InputFormat,
    ) -> Result<Self, ProjectComputationInputsError> {
        let reference = Self {
            id: id.into(),
            relative_path: relative_path.into(),
            format,
        };
        if !valid_input_id(&reference.id)
            || reference.relative_path.is_empty()
            || reference.relative_path.len() > MAX_RELATIVE_PATH_BYTES
            || reference.relative_path.as_bytes().contains(&0)
        {
            return Err(ProjectComputationInputsError::InvalidReference);
        }
        Ok(reference)
    }

    /// Returns the request-local input ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the untrusted workspace-relative path.
    #[must_use]
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    /// Returns the requested input representation.
    #[must_use]
    pub const fn format(&self) -> InputFormat {
        self.format
    }
}

/// Host-only provenance for one safely opened computation input.
#[derive(Clone, Eq, PartialEq)]
pub struct ProjectComputationInputBinding {
    id: String,
    relative_path: String,
    format: InputFormat,
    byte_size: u64,
    content_sha256: String,
    resource_revision: String,
}

impl ProjectComputationInputBinding {
    /// Returns the request-local input ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the exact validated workspace-relative path.
    #[must_use]
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    /// Returns the validated input representation.
    #[must_use]
    pub const fn format(&self) -> InputFormat {
        self.format
    }

    /// Returns the exact number of bytes supplied to the runner.
    #[must_use]
    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }

    /// Returns SHA-256 of the exact bytes supplied to the runner.
    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    /// Returns the content-addressed observed resource revision.
    #[must_use]
    pub fn resource_revision(&self) -> &str {
        &self.resource_revision
    }
}

/// Immutable inputs and authority binding prepared for one project computation.
#[derive(Clone, Eq, PartialEq)]
pub struct ProjectComputationInputSnapshot {
    project_id: ProjectId,
    grant_id: WorkspaceGrantId,
    capability_revision: String,
    inputs: Vec<ComputationInput>,
    bindings: Vec<ProjectComputationInputBinding>,
}

impl ProjectComputationInputSnapshot {
    /// Returns the opaque project identity used for admission.
    #[must_use]
    pub const fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    /// Returns the opaque workspace capability identity used for admission.
    #[must_use]
    pub const fn grant_id(&self) -> &WorkspaceGrantId {
        &self.grant_id
    }

    /// Returns the host-derived revision of the project workspace capability.
    #[must_use]
    pub fn capability_revision(&self) -> &str {
        &self.capability_revision
    }

    /// Returns host-only input provenance in request order.
    #[must_use]
    pub fn bindings(&self) -> &[ProjectComputationInputBinding] {
        &self.bindings
    }
}

/// Typed rejection while binding or admitting project computation inputs.
#[derive(Debug)]
pub enum ProjectComputationInputsError {
    /// An input ID or relative-path bound was invalid.
    InvalidReference,
    /// Two inputs used the same request-local ID.
    DuplicateInputId,
    /// Two inputs named the same strict workspace-relative path.
    DuplicatePath,
    /// The input count or aggregate byte size exceeded product policy.
    InputLimit,
    /// The resolved bookmark root did not identify the persisted granted root.
    WorkspaceRootMismatch,
    /// The project attachment or workspace capability changed before admission.
    AuthorityChanged,
    /// Stable handle-relative opening or reading failed safely.
    Filesystem(FilesystemToolError),
    /// Input encoding or structure failed the computation contract.
    Contract(ContractError),
}

impl fmt::Display for ProjectComputationInputsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReference => formatter.write_str("project computation input is invalid"),
            Self::DuplicateInputId => formatter.write_str("computation input ID is duplicated"),
            Self::DuplicatePath => formatter.write_str("computation input path is duplicated"),
            Self::InputLimit => formatter.write_str("project computation input limit was exceeded"),
            Self::WorkspaceRootMismatch => {
                formatter.write_str("resolved workspace does not match the active grant")
            }
            Self::AuthorityChanged => {
                formatter.write_str("project computation authority changed before admission")
            }
            Self::Filesystem(error) => error.fmt(formatter),
            Self::Contract(error) => error.fmt(formatter),
        }
    }
}

impl Error for ProjectComputationInputsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Filesystem(error) => Some(error),
            Self::Contract(error) => Some(error),
            Self::InvalidReference
            | Self::DuplicateInputId
            | Self::DuplicatePath
            | Self::InputLimit
            | Self::WorkspaceRootMismatch
            | Self::AuthorityChanged => None,
        }
    }
}

impl AppCore {
    /// Safely snapshots project files into pathless computation inputs.
    ///
    /// The operation holds the workspace read gate while it verifies current
    /// project ownership and read permission, matches the resolved bookmark
    /// root to the persisted grant, and opens every path through FS-003.
    ///
    /// # Errors
    ///
    /// Returns an error when authority is absent, a root or path is unsafe, an
    /// input changes during its read, or any input/aggregate contract fails.
    pub fn bind_project_computation_inputs(
        &self,
        project_id: &ProjectId,
        grant_id: &WorkspaceGrantId,
        resolved_root: impl AsRef<Path>,
        references: &[ProjectComputationInputReference],
    ) -> Result<ProjectComputationInputSnapshot, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        if references.is_empty() || references.len() > MAX_INPUTS {
            return Err(ProjectComputationInputsError::InputLimit.into());
        }
        let grant = self.current_project_computation_grant(project_id, grant_id)?;
        let workspace = self
            .open_workspace(resolved_root)
            .map_err(FilesystemToolError::from)
            .map_err(ProjectComputationInputsError::Filesystem)?;
        let persisted_workspace = self
            .open_workspace(&grant.canonical_root)
            .map_err(FilesystemToolError::from)
            .map_err(ProjectComputationInputsError::Filesystem)?;
        if workspace.identity() != persisted_workspace.identity() {
            return Err(ProjectComputationInputsError::WorkspaceRootMismatch.into());
        }

        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut total_bytes = 0_usize;
        let mut inputs = Vec::with_capacity(references.len());
        let mut bindings = Vec::with_capacity(references.len());
        for reference in references {
            if !ids.insert(reference.id.as_str()) {
                return Err(ProjectComputationInputsError::DuplicateInputId.into());
            }
            if !paths.insert(reference.relative_path.as_str()) {
                return Err(ProjectComputationInputsError::DuplicatePath.into());
            }
            let read =
                filesystem_tools::read_bytes(&workspace, &reference.relative_path, MAX_INPUT_BYTES)
                    .map_err(ProjectComputationInputsError::Filesystem)?;
            total_bytes = total_bytes
                .checked_add(read.bytes.len())
                .ok_or(ProjectComputationInputsError::InputLimit)?;
            if total_bytes > MAX_TOTAL_INPUT_BYTES {
                return Err(ProjectComputationInputsError::InputLimit.into());
            }
            let input = ComputationInput::new(&reference.id, reference.format, &read.bytes)
                .map_err(ProjectComputationInputsError::Contract)?;
            let resource_revision = read.content_sha256.clone();
            bindings.push(ProjectComputationInputBinding {
                id: reference.id.clone(),
                relative_path: read.relative_path,
                format: reference.format,
                byte_size: read.byte_size,
                content_sha256: read.content_sha256,
                resource_revision,
            });
            inputs.push(input);
        }

        Ok(ProjectComputationInputSnapshot {
            project_id: project_id.clone(),
            grant_id: grant_id.clone(),
            capability_revision: capability_revision(project_id, &grant),
            inputs,
            bindings,
        })
    }

    /// Runs a trusted launch operation under the revalidated project capability.
    ///
    /// The workspace read gate remains held for the complete callback, closing
    /// the authority race between revalidation and runner startup. The callback
    /// receives the only public access to the pathless runner inputs.
    ///
    /// # Errors
    ///
    /// Returns `AuthorityChanged` when the project attachment, bookmark, root,
    /// permission or durable grant state no longer matches the snapshot.
    pub fn with_admitted_project_computation_inputs<T>(
        &self,
        snapshot: &ProjectComputationInputSnapshot,
        launch: impl FnOnce(&[ComputationInput]) -> T,
    ) -> Result<T, AppCoreError> {
        let _workspace_guard = self.read_workspace_operations()?;
        let grant = match self
            .current_project_computation_grant(&snapshot.project_id, &snapshot.grant_id)
        {
            Ok(grant) => grant,
            Err(AppCoreError::WorkspaceGrantNotFound | AppCoreError::WorkspaceAuthorization(_)) => {
                return Err(ProjectComputationInputsError::AuthorityChanged.into());
            }
            Err(error) => return Err(error),
        };
        if capability_revision(&snapshot.project_id, &grant) != snapshot.capability_revision {
            return Err(ProjectComputationInputsError::AuthorityChanged.into());
        }
        Ok(launch(&snapshot.inputs))
    }

    fn current_project_computation_grant(
        &self,
        project_id: &ProjectId,
        grant_id: &WorkspaceGrantId,
    ) -> Result<WorkspaceGrant, AppCoreError> {
        let grant = self.with_storage(|storage| {
            if !storage.project_has_workspace(project_id, grant_id)? {
                return Ok(None);
            }
            storage.workspace_grant(grant_id)
        })?;
        let grant = grant.ok_or(AppCoreError::WorkspaceGrantNotFound)?;
        filesystem_tools::authorize_workspace_operation(
            grant.permission_level,
            WorkspaceOperation::Read,
        )
        .map_err(AppCoreError::WorkspaceAuthorization)?;
        Ok(grant)
    }
}

fn capability_revision(project_id: &ProjectId, grant: &WorkspaceGrant) -> String {
    let permission = match grant.permission_level {
        WorkspacePermissionLevel::ReadOnly => b"read-only".as_slice(),
        WorkspacePermissionLevel::ReadAndWrite => b"read-write".as_slice(),
    };
    let mut digest = Sha256::new();
    for field in [
        b"zeta-project-computation-capability-v1".as_slice(),
        project_id.0.as_bytes(),
        grant.id.0.as_bytes(),
        grant.canonical_root.as_bytes(),
        grant.created_at.as_bytes(),
        permission,
        &grant.bookmark_data,
    ] {
        digest.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_le_bytes());
        digest.update(field);
    }
    format!("{:x}", digest.finalize())
}

fn valid_input_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphabetic()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs, os::unix::fs::symlink};

    use computation_runtime::{ComputationRequest, OutputExpectation, OutputKind};
    use tempfile::tempdir;

    use super::*;

    const NOW: &str = "2026-08-21T12:00:00Z";

    type TestFixture = (
        tempfile::TempDir,
        tempfile::TempDir,
        AppCore,
        ProjectId,
        WorkspaceGrantId,
    );

    fn app_and_workspace() -> Result<TestFixture, Box<dyn Error>> {
        let data = tempdir()?;
        let workspace = tempdir()?;
        let app = AppCore::initialize(data.path())?;
        let project = app.create_project("Computation", NOW)?;
        let attached =
            app.add_project_workspace(&project.id, workspace.path(), vec![1, 2, 3], NOW)?;
        Ok((data, workspace, app, project.id, attached.grant_id))
    }

    #[test]
    fn pure_computation_needs_no_project_folder_grant() -> Result<(), Box<dyn Error>> {
        let data = tempdir()?;
        let app = AppCore::initialize(data.path())?;
        let project = app.create_project("Pure", NOW)?;
        assert!(app.project_workspaces(&project.id)?.is_empty());

        let request = ComputationRequest::new(
            "pure-without-grant",
            "export default function compute() { return 42; }",
            Vec::new(),
            OutputExpectation::new(OutputKind::Json, 64, 1)?,
        )?;
        assert!(request.inputs().is_empty());
        Ok(())
    }

    #[test]
    fn binds_typed_files_to_exact_pathless_digest_bound_inputs() -> Result<(), Box<dyn Error>> {
        let (_data, workspace, app, project_id, grant_id) = app_and_workspace()?;
        fs::write(workspace.path().join("notes.txt"), "hello")?;
        fs::write(workspace.path().join("rows.json"), br#"[{"value":2}]"#)?;
        fs::write(workspace.path().join("bytes.bin"), [0_u8, 1, 2])?;
        let references = [
            ProjectComputationInputReference::new("notes", "notes.txt", InputFormat::Utf8)?,
            ProjectComputationInputReference::new("rows", "rows.json", InputFormat::Json)?,
            ProjectComputationInputReference::new("blob", "bytes.bin", InputFormat::Bytes)?,
        ];

        let snapshot = app.bind_project_computation_inputs(
            &project_id,
            &grant_id,
            workspace.path(),
            &references,
        )?;
        let request = app.with_admitted_project_computation_inputs(&snapshot, |inputs| {
            ComputationRequest::new(
                "project-snapshot",
                "export default function compute(zeta) { return zeta.inputText('notes'); }",
                inputs.to_vec(),
                OutputExpectation::new(OutputKind::Text, 64, 1)?,
            )
        })??;
        assert_eq!(snapshot.bindings().len(), 3);
        assert_eq!(snapshot.bindings()[0].relative_path(), "notes.txt");
        assert_eq!(snapshot.bindings()[0].byte_size(), 5);
        assert_eq!(
            snapshot.bindings()[0].content_sha256(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(
            snapshot.bindings()[0].resource_revision(),
            snapshot.bindings()[0].content_sha256()
        );
        assert_eq!(snapshot.capability_revision().len(), 64);
        assert_eq!(request.inputs().len(), 3);
        Ok(())
    }

    #[test]
    fn rejects_unattached_projects_and_substituted_roots() -> Result<(), Box<dyn Error>> {
        let (_data, workspace, app, project_id, grant_id) = app_and_workspace()?;
        let outside = tempdir()?;
        fs::write(workspace.path().join("safe.txt"), "inside")?;
        fs::write(outside.path().join("safe.txt"), "outside")?;
        let reference = [ProjectComputationInputReference::new(
            "safe",
            "safe.txt",
            InputFormat::Utf8,
        )?];
        let other_project = app.create_project("Other", "2026-08-21T12:01:00Z")?;

        assert!(matches!(
            app.bind_project_computation_inputs(
                &other_project.id,
                &grant_id,
                workspace.path(),
                &reference,
            ),
            Err(AppCoreError::WorkspaceGrantNotFound)
        ));
        assert!(matches!(
            app.bind_project_computation_inputs(&project_id, &grant_id, outside.path(), &reference),
            Err(AppCoreError::ProjectComputation(
                ProjectComputationInputsError::WorkspaceRootMismatch
            ))
        ));
        Ok(())
    }

    #[test]
    fn bridge_preserves_fs003_path_rejections() -> Result<(), Box<dyn Error>> {
        let (_data, workspace, app, project_id, grant_id) = app_and_workspace()?;
        let outside = tempdir()?;
        fs::write(workspace.path().join("Case.txt"), "inside")?;
        fs::write(outside.path().join("secret.txt"), "outside")?;
        symlink(
            outside.path().join("secret.txt"),
            workspace.path().join("linked.txt"),
        )?;

        for path in ["../secret.txt", "./Case.txt", "case.txt", "linked.txt"] {
            let reference = [ProjectComputationInputReference::new(
                "input",
                path,
                InputFormat::Utf8,
            )?];
            assert!(matches!(
                app.bind_project_computation_inputs(
                    &project_id,
                    &grant_id,
                    workspace.path(),
                    &reference,
                ),
                Err(AppCoreError::ProjectComputation(
                    ProjectComputationInputsError::Filesystem(_)
                ))
            ));
        }
        fs::write(workspace.path().join("invalid.json"), "{not-json}")?;
        let invalid_json = [ProjectComputationInputReference::new(
            "json",
            "invalid.json",
            InputFormat::Json,
        )?];
        assert!(matches!(
            app.bind_project_computation_inputs(
                &project_id,
                &grant_id,
                workspace.path(),
                &invalid_json,
            ),
            Err(AppCoreError::ProjectComputation(
                ProjectComputationInputsError::Contract(_)
            ))
        ));
        Ok(())
    }

    #[test]
    fn duplicate_references_and_changed_authority_fail_closed() -> Result<(), Box<dyn Error>> {
        let (_data, workspace, app, project_id, grant_id) = app_and_workspace()?;
        fs::write(workspace.path().join("a.txt"), "first")?;
        fs::write(workspace.path().join("b.txt"), "second")?;
        let duplicate_ids = [
            ProjectComputationInputReference::new("same", "a.txt", InputFormat::Utf8)?,
            ProjectComputationInputReference::new("same", "b.txt", InputFormat::Utf8)?,
        ];
        let duplicate_paths = [
            ProjectComputationInputReference::new("first", "a.txt", InputFormat::Utf8)?,
            ProjectComputationInputReference::new("second", "a.txt", InputFormat::Utf8)?,
        ];
        assert!(matches!(
            app.bind_project_computation_inputs(
                &project_id,
                &grant_id,
                workspace.path(),
                &duplicate_ids,
            ),
            Err(AppCoreError::ProjectComputation(
                ProjectComputationInputsError::DuplicateInputId
            ))
        ));
        assert!(matches!(
            app.bind_project_computation_inputs(
                &project_id,
                &grant_id,
                workspace.path(),
                &duplicate_paths,
            ),
            Err(AppCoreError::ProjectComputation(
                ProjectComputationInputsError::DuplicatePath
            ))
        ));

        let reference = [ProjectComputationInputReference::new(
            "input",
            "a.txt",
            InputFormat::Utf8,
        )?];
        let snapshot = app.bind_project_computation_inputs(
            &project_id,
            &grant_id,
            workspace.path(),
            &reference,
        )?;
        app.update_project_workspace_permission(
            &project_id,
            &grant_id,
            WorkspacePermissionLevel::ReadAndWrite,
        )?;
        assert!(matches!(
            app.with_admitted_project_computation_inputs(&snapshot, |_| ()),
            Err(AppCoreError::ProjectComputation(
                ProjectComputationInputsError::AuthorityChanged
            ))
        ));

        let current = app.bind_project_computation_inputs(
            &project_id,
            &grant_id,
            workspace.path(),
            &reference,
        )?;
        assert!(app.remove_project_workspace(&project_id, &grant_id, "2026-08-21T12:02:00Z")?);
        assert!(matches!(
            app.with_admitted_project_computation_inputs(&current, |_| ()),
            Err(AppCoreError::ProjectComputation(
                ProjectComputationInputsError::AuthorityChanged
            ))
        ));
        Ok(())
    }

    #[test]
    fn later_file_changes_create_a_new_snapshot_without_mutating_the_old_one()
    -> Result<(), Box<dyn Error>> {
        let (_data, workspace, app, project_id, grant_id) = app_and_workspace()?;
        let path = workspace.path().join("data.txt");
        fs::write(&path, "before")?;
        let reference = [ProjectComputationInputReference::new(
            "data",
            "data.txt",
            InputFormat::Utf8,
        )?];
        let before = app.bind_project_computation_inputs(
            &project_id,
            &grant_id,
            workspace.path(),
            &reference,
        )?;
        fs::write(path, "after")?;
        let after = app.bind_project_computation_inputs(
            &project_id,
            &grant_id,
            workspace.path(),
            &reference,
        )?;

        assert_ne!(
            before.bindings()[0].content_sha256(),
            after.bindings()[0].content_sha256()
        );
        assert_eq!(before.bindings()[0].byte_size(), 6);
        assert_eq!(after.bindings()[0].byte_size(), 5);
        app.with_admitted_project_computation_inputs(&before, |_| ())?;
        Ok(())
    }
}
