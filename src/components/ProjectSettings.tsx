import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type {
  CatalogueModel,
  IpcError,
  ModelSelection,
  ProjectSummary,
  ProjectWorkspaceSummary,
  UpdateProjectWorkspacePermissionRequest,
  WorkspaceGrantId,
  WorkspacePermissionLevel,
} from "../generated/ipc";

type ProjectSettingsProps = {
  project: ProjectSummary;
  models: CatalogueModel[];
  defaultModel: ModelSelection | null;
};

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as IpcError).message);
  }
  return "Project folder access could not be updated.";
}

export function ProjectSettings({ project, models, defaultModel }: ProjectSettingsProps) {
  const [workspaces, setWorkspaces] = useState<ProjectWorkspaceSummary[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmingRemoveId, setConfirmingRemoveId] = useState<WorkspaceGrantId | null>(null);
  const [projectModel, setProjectModel] = useState<ModelSelection | null>(null);
  const [modelPending, setModelPending] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);
  const availableModels = models.filter((candidate) => candidate.installed && candidate.chatSelectable);
  const onlyModel = availableModels.length === 1 ? availableModels[0] : null;
  const showSingleModel = onlyModel !== null && (projectModel === null
    || (projectModel.modelId === onlyModel.id && projectModel.modelVersion === onlyModel.version));

  useEffect(() => {
    let mounted = true;
    setPending(true);
    setError(null);
    invoke<ProjectWorkspaceSummary[]>("list_project_workspaces", { projectId: project.id })
      .then((folders) => { if (mounted) setWorkspaces(folders); })
      .catch((loadError) => { if (mounted) setError(errorMessage(loadError)); })
      .finally(() => { if (mounted) setPending(false); });
    return () => { mounted = false; };
  }, [project.id]);

  useEffect(() => {
    let mounted = true;
    setModelError(null);
    invoke<ModelSelection | null>("project_model", { projectId: project.id })
      .then((selection) => { if (mounted) setProjectModel(selection); })
      .catch((loadError) => { if (mounted) setModelError(errorMessage(loadError)); });
    return () => { mounted = false; };
  }, [project.id]);

  async function updateProjectModel(value: string) {
    if (modelPending) return;
    const selection = value === "default"
      ? null
      : models
          .filter((candidate) => candidate.installed && candidate.chatSelectable)
          .map((candidate) => ({ modelId: candidate.id, modelVersion: candidate.version }))
          .find((candidate) => `${candidate.modelId}@${candidate.modelVersion}` === value) ?? null;
    if (value !== "default" && selection === null) {
      setModelError("That model is no longer available on this device.");
      return;
    }
    setModelPending(true);
    setModelError(null);
    try {
      const updated = await invoke<ModelSelection | null>("set_project_model", {
        projectId: project.id,
        selection,
      });
      setProjectModel(updated);
    } catch (updateError) {
      setModelError(errorMessage(updateError));
    } finally {
      setModelPending(false);
    }
  }

  async function addFolder() {
    if (pending) return;
    setPending(true);
    setError(null);
    setConfirmingRemoveId(null);
    try {
      const workspace = await invoke<ProjectWorkspaceSummary | null>("add_project_workspace", {
        projectId: project.id,
      });
      if (workspace) {
        setWorkspaces((current) => [
          ...current.filter((item) => item.grantId !== workspace.grantId),
          workspace,
        ].sort((left, right) => left.role === right.role ? 0 : left.role === "primary" ? -1 : 1));
      }
    } catch (addError) {
      setError(errorMessage(addError));
    } finally {
      setPending(false);
    }
  }

  async function removeFolder(grantId: WorkspaceGrantId) {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      const removed = await invoke<boolean>("remove_project_workspace", {
        projectId: project.id,
        grantId,
      });
      if (!removed) setError("This folder is no longer attached to the project.");
      const folders = await invoke<ProjectWorkspaceSummary[]>("list_project_workspaces", {
        projectId: project.id,
      });
      setWorkspaces(folders);
      setConfirmingRemoveId(null);
    } catch (removeError) {
      setError(errorMessage(removeError));
    } finally {
      setPending(false);
    }
  }

  async function updatePermission(
    grantId: WorkspaceGrantId,
    permissionLevel: WorkspacePermissionLevel,
  ) {
    if (pending) return;
    const request: UpdateProjectWorkspacePermissionRequest = {
      projectId: project.id,
      grantId,
      permissionLevel,
    };
    setPending(true);
    setError(null);
    try {
      const updated = await invoke<ProjectWorkspaceSummary>(
        "update_project_workspace_permission",
        { request },
      );
      setWorkspaces((current) => current.map((item) => item.grantId === grantId ? updated : item));
    } catch (updateError) {
      setError(errorMessage(updateError));
    } finally {
      setPending(false);
    }
  }

  return (
    <section className="project-settings" aria-labelledby="project-settings-title">
      <header className="project-settings-header">
        <div>
          <p className="eyebrow">Project settings</p>
          <h1 id="project-settings-title">{project.name}</h1>
          <p>Choose the folders and local model used by chats in this project.</p>
        </div>
      </header>

      <div className="settings-section-heading">
        <div><p className="eyebrow">Project folders</p><p>Folders are optional. Add any folders that chats in this project should be able to use.</p></div>
        <button type="button" onClick={addFolder} disabled={pending}><span aria-hidden="true">+</span>Add folder</button>
      </div>
      <div className="project-folder-card">
        <div className="project-folder-heading">
          <strong>Folder access</strong>
          <span>{workspaces.length} {workspaces.length === 1 ? "folder" : "folders"}</span>
        </div>
        <p className="project-folder-guidance">Read and write lets the AI change project files without approving each edit. Review changes before running project code. Credential files and protected configuration paths stay blocked.</p>
        {workspaces.length > 0 ? (
          <ul className="project-folder-list">
            {workspaces.map((workspace) => (
              <li key={workspace.grantId}>
                <div className="project-folder-identity">
                  <strong>{workspace.displayName}</strong>
                  <span>{workspace.role === "primary" ? "Default folder" : "Additional folder"}</span>
                </div>
                <select
                  className="workspace-permission-select"
                  value={workspace.permissionLevel}
                  onChange={(event) => updatePermission(
                    workspace.grantId,
                    event.target.value as WorkspacePermissionLevel,
                  )}
                  onPointerDown={(event) => { event.currentTarget.dataset.pointerFocus = "true"; }}
                  onBlur={(event) => { delete event.currentTarget.dataset.pointerFocus; }}
                  disabled={pending}
                  aria-label={`Permission for ${workspace.displayName}`}
                >
                  <option value="readOnly">Read only</option>
                  <option value="readAndWrite">Read and write</option>
                </select>
                {confirmingRemoveId === workspace.grantId ? (
                  <div className="workspace-revoke-confirm" aria-label={`Remove ${workspace.displayName}?`}>
                    <button type="button" onClick={() => removeFolder(workspace.grantId)} disabled={pending}>Remove</button>
                    <button type="button" onClick={() => setConfirmingRemoveId(null)} disabled={pending}>Cancel</button>
                  </div>
                ) : (
                  <button className="workspace-revoke" type="button" onClick={() => setConfirmingRemoveId(workspace.grantId)} disabled={pending}>
                    Remove
                  </button>
                )}
              </li>
            ))}
          </ul>
        ) : (
          <div className="project-folder-empty">
            <p>No folders are currently available to this project.</p>
            <button type="button" onClick={addFolder} disabled={pending}>Choose a folder</button>
          </div>
        )}
        {error ? <p className="workspace-error" role="alert">{error}</p> : null}
      </div>

      <div className="settings-section-heading project-model-heading">
        <div><p className="eyebrow">Project model</p><p>New chats use this downloaded model. Existing chats keep their current model.</p></div>
      </div>
      <div className="project-model-card">
        {showSingleModel && onlyModel ? <>
          <span className="project-model-label">Model for new chats</span>
          <div className="project-single-model">
            <strong>{onlyModel.displayName}</strong>
            <span>{projectModel === null ? "Uses the Minutus default" : "Selected for this project"}</span>
            {projectModel !== null ? <button type="button" disabled={modelPending} onClick={() => void updateProjectModel("default")}>Use Minutus default</button> : null}
          </div>
        </> : <>
        <label htmlFor="project-model-select">Model for new chats</label>
        <select
          id="project-model-select"
          value={projectModel ? `${projectModel.modelId}@${projectModel.modelVersion}` : "default"}
          onChange={(event) => void updateProjectModel(event.target.value)}
          disabled={modelPending}
        >
          <option value="default">Use Minutus default{defaultModel ? ` · ${models.find((candidate) => candidate.id === defaultModel.modelId && candidate.version === defaultModel.modelVersion)?.displayName ?? "Downloaded model"}` : ""}</option>
          {projectModel && !availableModels.some((candidate) => candidate.id === projectModel.modelId && candidate.version === projectModel.modelVersion) ? <option disabled value={`${projectModel.modelId}@${projectModel.modelVersion}`}>Saved model · Unavailable</option> : null}
          {availableModels.map((candidate) => (
            <option key={`${candidate.id}@${candidate.version}`} value={`${candidate.id}@${candidate.version}`}>
              {candidate.displayName}{candidate.recommended ? " · Recommended" : ""}
            </option>
          ))}
        </select>
        </>}
        {modelError ? <p className="workspace-error" role="alert">{modelError}</p> : null}
      </div>
    </section>
  );
}
