import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import type { CatalogueModel, CatalogueModule, ExternalModelProgress } from "../../generated/ipc";
import { modelFileErrorMessage } from "./modelFileErrors";
import { moduleSetupReadiness, moduleStatusLabel } from "./moduleReadiness";

type Props = {
  model: CatalogueModel; disabled: boolean; onChanged: () => Promise<void>; children?: ReactNode;
  settings?: boolean; setupActive?: boolean; onSetupStarted?: () => void; onSetupFinished?: () => void; onBusyChange?: (busy: boolean) => void;
};
type Operation = { artifactId: string; kind: "import" | "download" };

export function ModelFileActions({ model, disabled, onChanged, children, settings = false, setupActive = false, onSetupStarted, onSetupFinished, onBusyChange }: Props) {
  const [locallyExpanded, setExpanded] = useState(false);
  const expanded = locallyExpanded || (!settings && (setupActive || model.externalFiles));
  const [operation, setOperation] = useState<Operation | null>(null);
  const [removing, setRemoving] = useState(false);
  const [progress, setProgress] = useState<ExternalModelProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const busy = operation !== null || removing;
  const readiness = moduleSetupReadiness(model.modules);
  const missingModules = model.modules.filter((module) => module.registrationState !== "available");
  const selection = { modelId: model.id, modelVersion: model.version };

  useEffect(() => {
    if (expanded && !busy && readiness.allReady && onSetupFinished) onSetupFinished();
  }, [expanded, busy, readiness.allReady, onSetupFinished]);

  async function run(modules: CatalogueModule | CatalogueModule[], kind: Operation["kind"]) {
    if (busy || disabled) return;
    const pending = Array.isArray(modules) ? modules : [modules];
    if (pending.length === 0) return;
    onSetupStarted?.();
    onBusyChange?.(true);
    setOperation({ artifactId: pending[0].artifactId, kind });
    setError(null);
    setProgress(null);
    let unlisten: (() => void) | undefined;
    try {
      for (const module of pending) {
        setOperation({ artifactId: module.artifactId, kind });
        setProgress(null);
        unlisten = await listen<ExternalModelProgress>("external-model-progress", ({ payload }) => {
          if (payload.modelId === model.id && payload.artifactId === module.artifactId) setProgress(payload);
        });
        await invoke(kind === "import" ? "register_external_model" : "download_model_module", { selection, artifactId: module.artifactId });
        unlisten();
        unlisten = undefined;
        await onChanged();
      }
    } catch (failure) {
      setError(modelFileErrorMessage(failure));
    } finally {
      unlisten?.();
      await onChanged().catch(() => setError("Could not refresh module availability. Reopen Settings to retry."));
      setOperation(null);
      setProgress(null);
      onBusyChange?.(false);
    }
  }

  async function remove() {
    if (busy || disabled) return;
    setRemoving(true);
    setError(null);
    onBusyChange?.(true);
    try { await invoke("forget_external_model", { selection }); }
    catch (failure) { setError(modelFileErrorMessage(failure)); }
    finally {
      await onChanged().catch(() => setError("Could not refresh module availability."));
      setRemoving(false);
      onBusyChange?.(false);
    }
  }

  if (!model.localImportSupported) return children ? <div className="model-import">{children}</div> : null;
  if (settings && model.installed && readiness.allReady && !model.externalFiles && !expanded) return null;
  return <div className="model-import model-file-actions">
    {expanded ? <div className="model-module-list" aria-label="Model setup modules">
      {model.modules.map((module) => {
        const ready = module.registrationState === "available";
        const active = operation?.artifactId === module.artifactId;
        return <section className="model-module" key={module.artifactId} aria-label={`${module.displayName} — ${module.required ? "Required" : "Optional"}`}>
          <div className="model-module-heading">
            <strong>{module.displayName}</strong>
            <span className="model-module-requirement" data-required={module.required}>{module.required ? "Required" : "Optional"}</span>
          </div>
          <small className="model-module-filename">{module.fileName}</small>
          <span className={ready ? "setting-status configured" : "setting-status"} role="status">
            {active ? progress ? progress.phase === "downloading" ? "Downloading…" : "Verifying SHA-256…" : operation?.kind === "import" ? "Choose this module’s file…" : "Starting download…" : moduleStatusLabel(module)}
          </span>
          {!ready ? <div className="model-module-actions">
            <button className="secondary-action" type="button" disabled={disabled || busy} onClick={() => void run(module, "import")}>
              {module.registrationState === "unavailable" || module.registrationState === "verificationFailed" ? "Locate local files" : "Import from local files"}
            </button>
            <button className="secondary-action" type="button" disabled={disabled || busy} onClick={() => void run(module, "download")} aria-label={`Download ${module.displayName} — ${module.required ? "Required" : "Optional"}`}>
              Download · {(module.downloadBytes / 1024 ** 3 >= 1 ? `${(module.downloadBytes / 1024 ** 3).toFixed(1)} GiB` : `${(module.downloadBytes / 1024 ** 2).toFixed(0)} MiB`)}
            </button>
          </div> : null}
          {active && progress ? <progress aria-label={`${module.displayName} ${progress.phase}`} max={progress.totalBytes} value={progress.verifiedBytes} /> : null}
          {active && operation?.kind === "download" && progress?.phase === "downloading" ? <button className="secondary-action" type="button" onClick={() => void invoke("cancel_model_download")}>Cancel download</button> : null}
        </section>;
      })}
      <p className="phase-note">Every Required module must be verified to finish setup. Optional modules can be added later in Settings. Imported files stay in place and are never copied, moved or deleted.</p>
    </div> : null}
    <div className="model-file-action-buttons">
      {!busy && !expanded ? children : null}
      {!expanded && (!settings || !model.installed || !readiness.allReady) ? <button className="secondary-action" type="button" disabled={disabled || busy} onClick={() => { onSetupStarted?.(); setExpanded(true); }}>{model.installed ? "Manage modules" : "Import local files"}</button> : null}
      {settings && expanded ? <button className="secondary-action" type="button" disabled={disabled || busy} onClick={() => setExpanded(false)}>Done</button> : null}
      {expanded && onSetupFinished ? <>
        {missingModules.some((module) => !module.required) ? <button className="primary-action" type="button" disabled={disabled || busy} onClick={() => void run(missingModules, "download")}>Download all modules</button> : null}
        <button className="primary-action" type="button" disabled={disabled || busy} onClick={() => readiness.requiredReady ? onSetupFinished() : void run(missingModules.filter((module) => module.required), "download")}>{readiness.requiredReady ? "Finish setup" : "Download required modules"}</button>
      </> : null}
      {settings && model.externalFiles ? <button className="secondary-action" type="button" disabled={disabled || busy} onClick={() => void remove()}>Remove from Minutus</button> : null}
    </div>
    {error ? <p className="download-error" role="alert">{error}</p> : null}
  </div>;
}
