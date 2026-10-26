import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { modelAvailabilityLabel } from "./components/models/availability";
import { ChatWorkspace } from "./components/ChatWorkspace";
import { LaunchSplash } from "./components/LaunchSplash";
import type { ActiveChatResponse } from "./components/chat/generationActivity";
import { CreateProjectDialog } from "./components/CreateProjectDialog";
import { ProjectSettings } from "./components/ProjectSettings";
import { ProjectSidebar } from "./components/ProjectSidebar";
import { SpeechModelSettings } from "./components/SpeechModelSettings";
import { WebSearchSettings } from "./components/WebSearchSettings";
import { ReleaseUpdateSettings } from "./components/ReleaseUpdateSettings";
import { ThemeSettings, useThemePreference } from "./components/ThemeSettings";
import { EvmRpcSettingsPanel } from "./components/EvmRpcSettings";
import { WalletWorkspace } from "./components/wallet/WalletWorkspace";
import { useWalletState } from "./components/wallet/useWalletState";
import type {
  AppStatus,
  CatalogueModel,
  Conversation,
  CreateProjectRequest,
  IpcError,
  ModelDownloadEvent,
  ModelDownloadPhase,
  ModelInstallReceipt,
  ModelPreferences,
  ProjectSummary,
  PublicWalletState,
} from "./generated/ipc";

const ModelFileActions = lazy(() => import("./components/models/ModelFileActions").then((module) => ({ default: module.ModelFileActions })));

type RuntimeStatus = "connecting" | "ready" | "preview";
type DownloadState = "idle" | "downloading" | "installed" | "failed";
type VisibleDownloadPhase = "preparing" | ModelDownloadPhase;

type TransferMetrics = {
  bytesPerSecond: number;
  etaSeconds: number;
};

type ProgressSample = {
  bytes: number;
  recordedAt: number;
};

const GIBIBYTE = 1024 ** 3;
const MEBIBYTE = 1024 ** 2;
const LAST_PROJECT_KEY = "zeta.lastProjectId";

function formatSize(bytes: number): string {
  if (bytes < GIBIBYTE) return `${(bytes / MEBIBYTE).toFixed(0)} MiB`;
  return `${(bytes / GIBIBYTE).toFixed(1)} GiB`;
}

function formatRate(bytesPerSecond: number): string {
  return `${formatSize(bytesPerSecond)}/s`;
}

function formatDuration(seconds: number): string {
  if (seconds < 60) return "Less than a minute";
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `About ${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes === 0
    ? `About ${hours} hr`
    : `About ${hours} hr ${remainingMinutes} min`;
}

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as IpcError).message);
  }
  return "The model operation could not be completed.";
}

function downloadErrorMessage(error: unknown): string {
  if (typeof error !== "object" || error === null || !("message" in error)) {
    return errorMessage(error);
  }
  const ipcError = error as IpcError;
  if (ipcError.code === "cancelled") {
    return "Download paused. Your downloaded bytes were kept; Retry download will resume from them.";
  }
  if (ipcError.message.includes("SHA-256")) {
    return `${ipcError.message}. The rejected bytes were cleared and no model was installed.`;
  }
  if (ipcError.retryable) {
    return `${ipcError.message}. Any valid partial download was kept for retry.`;
  }
  return ipcError.message;
}

function phaseLabel(phase: VisibleDownloadPhase, resumed: boolean): string {
  switch (phase) {
    case "preparing":
      return "Preparing secure download…";
    case "downloading":
      return resumed ? "Resuming download…" : "Downloading…";
    case "verifying":
      return "Verifying SHA-256…";
    case "installing":
      return "Installing verified model…";
  }
}

export function App() {
  const theme = useThemePreference();
  const [launchVisible, setLaunchVisible] = useState(true);
  const [runtimeStatus, setRuntimeStatus] = useState<RuntimeStatus>("connecting");
  const [activeConversation, setActiveConversation] = useState<Conversation | null>(null);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [activeProject, setActiveProject] = useState<ProjectSummary | null>(null);
  const [projectPending, setProjectPending] = useState(false);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [projectCreatorOpen, setProjectCreatorOpen] = useState(false);
  const [showProjectSettings, setShowProjectSettings] = useState(false);
  const [showAppSettings, setShowAppSettings] = useState(false);
  const [showWallet, setShowWallet] = useState(false);
  const [wallet, setWallet] = useWalletState(showWallet);
  const [conversationHistoryVersion, setConversationHistoryVersion] = useState(0);
  const [activeChatResponse, setActiveChatResponse] = useState<ActiveChatResponse | null>(null);
  const [moduleSetupActive, setModuleSetupActive] = useState(false);
  const [moduleOperationBusy, setModuleOperationBusy] = useState(false);
  const finishModuleSetup = useCallback(() => setModuleSetupActive(false), []);
  const [models, setModels] = useState<CatalogueModel[]>([]);
  const [modelPreferences, setModelPreferences] = useState<ModelPreferences>({ defaultModel: null });
  const [modelPreferenceError, setModelPreferenceError] = useState<string | null>(null);
  const [selectedModelId, setSelectedModelId] = useState<string | null>(null);
  const [downloadState, setDownloadState] = useState<DownloadState>("idle");
  const [progress, setProgress] = useState<ModelDownloadEvent | null>(null);
  const [downloadPhase, setDownloadPhase] = useState<VisibleDownloadPhase>("preparing");
  const [transferMetrics, setTransferMetrics] = useState<TransferMetrics | null>(null);
  const [cancelRequested, setCancelRequested] = useState(false);
  const [progressListenerReady, setProgressListenerReady] = useState(false);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const progressSample = useRef<ProgressSample | null>(null);
  const modelRefresh = useRef<Promise<void> | null>(null);

  const finishLaunch = useCallback(() => setLaunchVisible(false), []);

  const refreshModels = useCallback(() => {
    // Focus and interval refreshes share one pending query if a provider is busy.
    if (modelRefresh.current) return modelRefresh.current;
    const pending = Promise.allSettled([
      invoke<CatalogueModel[]>("model_catalogue"), invoke<ModelPreferences>("model_preferences"),
    ]).then(([catalogueResult, preferencesResult]) => {
      // Wait for both even if one fails, so its still-running sibling cannot
      // accumulate behind generation on subsequent interval ticks.
      if (catalogueResult.status === "rejected") throw catalogueResult.reason;
      if (preferencesResult.status === "rejected") throw preferencesResult.reason;
      const catalogue = catalogueResult.value;
      const preferences = preferencesResult.value;
      setModels((previous) => JSON.stringify(previous) === JSON.stringify(catalogue) ? previous : catalogue);
      setModelPreferences((previous) => JSON.stringify(previous) === JSON.stringify(preferences) ? previous : preferences);
    }).finally(() => { modelRefresh.current = null; });
    modelRefresh.current = pending;
    return pending;
  }, []);

  useEffect(() => {
    if (runtimeStatus !== "ready") return;
    const refresh = () => { void refreshModels().catch(() => undefined); };
    window.addEventListener("focus", refresh);
    const interval = window.setInterval(refresh, 15_000);
    if (showAppSettings) refresh();
    return () => { window.removeEventListener("focus", refresh); window.clearInterval(interval); };
  }, [runtimeStatus, showAppSettings, refreshModels]);

  useEffect(() => {
    let mounted = true;

    invoke<AppStatus>("app_status")
      .then((status) => {
        if (!mounted || status.state !== "ready") return;
        setRuntimeStatus("ready");
        void Promise.allSettled([
          invoke<CatalogueModel[]>("model_catalogue"),
          invoke<ModelPreferences>("model_preferences"),
          invoke<ProjectSummary[]>("list_projects"),
          invoke<PublicWalletState>("wallet_state"),
        ]).then(([catalogueResult, preferencesResult, projectsResult, walletResult]) => {
          if (!mounted) return;
          if (catalogueResult.status === "fulfilled") {
            const catalogue = catalogueResult.value;
            setModels(catalogue);
            const initial = catalogue.find((item) => item.recommended) ?? catalogue[0];
            setSelectedModelId(initial?.id ?? null);
            if (initial?.installed) setDownloadState("installed");
          } else {
            setModelPreferenceError("The native model catalogue is temporarily unavailable.");
          }
          if (preferencesResult.status === "fulfilled") {
            setModelPreferences(preferencesResult.value);
          } else {
            setModelPreferenceError("Native model preferences are temporarily unavailable.");
          }
          if (projectsResult.status === "fulfilled") {
            const availableProjects = projectsResult.value;
            setProjects(availableProjects);
            const rememberedId = localStorage.getItem(LAST_PROJECT_KEY);
            const remembered = availableProjects.find((project) => project.id === rememberedId);
            setActiveProject(remembered ?? availableProjects[0] ?? null);
          }
          if (walletResult.status === "fulfilled") setWallet(walletResult.value);
        });
      })
      .catch(() => {
        if (mounted) setRuntimeStatus("preview");
      });

    return () => {
      mounted = false;
    };
  }, []);

  useEffect(() => {
    let mounted = true;
    let stopListening: (() => void) | undefined;

    listen<ModelDownloadEvent>("model-download-progress", (event) => {
      if (!mounted) return;
      const next = event.payload;
      setProgress(next);
      setDownloadPhase(next.phase);
      if (next.phase !== "downloading") {
        progressSample.current = null;
        setTransferMetrics(null);
        return;
      }

      const now = performance.now();
      const previous = progressSample.current;
      if (previous && next.aggregateDownloadedBytes > previous.bytes && now > previous.recordedAt) {
        const bytesPerSecond =
          ((next.aggregateDownloadedBytes - previous.bytes) * 1000) / (now - previous.recordedAt);
        setTransferMetrics({
          bytesPerSecond,
          etaSeconds: next.aggregateRemainingBytes / bytesPerSecond,
        });
      }
      progressSample.current = { bytes: next.aggregateDownloadedBytes, recordedAt: now };
    })
      .then((unlisten) => {
        if (mounted) {
          stopListening = unlisten;
          setProgressListenerReady(true);
        } else unlisten();
      })
      .catch(() => {
        if (mounted) {
          setProgressListenerReady(false);
          setDownloadError("Live download progress is unavailable. Restart Minutus before downloading.");
        }
      });

    return () => {
      mounted = false;
      stopListening?.();
    };
  }, []);

  const orderedModels = useMemo(
    () => [...models].sort((left, right) =>
      Number(right.recommended) - Number(left.recommended)
      || left.displayName.localeCompare(right.displayName)),
    [models],
  );
  const model = models.find((item) => item.id === selectedModelId) ?? models[0];
  const percentage = useMemo(() => {
    if (!progress || progress.aggregateTotalBytes === 0) return 0;
    return Math.min(100, (progress.aggregateDownloadedBytes / progress.aggregateTotalBytes) * 100);
  }, [progress]);
  const modelReady = !moduleSetupActive && models.some((item) => item.installed && item.providerAvailable);
  const workspaceReady = modelReady || modelPreferences.defaultModel !== null || activeConversation !== null
    || models.some((item) => item.registrationState !== "notInstalled");
  const backLabel = !workspaceReady ? "Back to model setup"
    : activeProject ? "Back to project" : "Back to projects";
  const companionsNeedInstall = model?.companions.some((item) => !item.installed) ?? false;
  const updateActiveConversation = useCallback((conversation: Conversation) => {
    setActiveConversation(conversation);
  }, []);
  const updateConversationHistory = useCallback(() => {
    setConversationHistoryVersion((version) => version + 1);
  }, []);
  const updateGenerationActivity = useCallback((response: ActiveChatResponse | null) => {
    setActiveChatResponse(response);
  }, []);

  const selectProject = useCallback((project: ProjectSummary) => {
    setActiveProject(project);
    setActiveConversation(null);
    setShowProjectSettings(false);
    setShowAppSettings(false);
    setShowWallet(false);
    localStorage.setItem(LAST_PROJECT_KEY, project.id);
  }, []);

  function returnToWorkspace() {
    setShowProjectSettings(false);
    setShowAppSettings(false);
    setShowWallet(false);
  }

  async function chooseDefaultModel(candidate: CatalogueModel) {
    setModelPreferenceError(null);
    try {
      const preferences = await invoke<ModelPreferences>("set_default_model", {
        request: { selection: { modelId: candidate.id, modelVersion: candidate.version } },
      });
      setModelPreferences(preferences);
    } catch (error) {
      setModelPreferenceError(errorMessage(error));
    }
  }

  function openProjectCreator() {
    if (projectPending || runtimeStatus !== "ready") return;
    setProjectError(null);
    setProjectCreatorOpen(true);
  }

  async function createProject(name: string) {
    if (projectPending || runtimeStatus !== "ready") return;
    setProjectPending(true);
    setProjectError(null);
    try {
      const request: CreateProjectRequest = { name };
      const project = await invoke<ProjectSummary>("create_project", { request });
      setProjects((current) => [project, ...current.filter((item) => item.id !== project.id)]);
      selectProject(project);
      setProjectCreatorOpen(false);
    } catch (error) {
      setProjectError(errorMessage(error));
    } finally {
      setProjectPending(false);
    }
  }

  async function installModel(target: CatalogueModel | undefined = model) {
    if (!target || !progressListenerReady || downloadState === "downloading") return;
    setSelectedModelId(target.id);
    setDownloadState("downloading");
    setDownloadError(null);
    setProgress(null);
    setDownloadPhase("preparing");
    setTransferMetrics(null);
    setCancelRequested(false);
    progressSample.current = null;
    try {
      await invoke<ModelInstallReceipt>("download_catalogue_model", {
        request: {
          modelId: target.id,
          modelVersion: target.version,
          companionArtifactIds: [],
        },
      });
      const catalogue = await invoke<CatalogueModel[]>("model_catalogue");
      const preferences = await invoke<ModelPreferences>("model_preferences");
      setModels(catalogue);
      setModelPreferences(preferences);
      setDownloadState("installed");
      setCancelRequested(false);
    } catch (error) {
      setDownloadError(downloadErrorMessage(error));
      setDownloadState("failed");
      setCancelRequested(false);
    }
  }

  async function cancelDownload() {
    if (cancelRequested) return;
    setCancelRequested(true);
    const accepted = await invoke<boolean>("cancel_model_download").catch(() => false);
    if (!accepted) setCancelRequested(false);
  }


  return (
    <div className="app-shell">
      {launchVisible ? <LaunchSplash onFinished={finishLaunch} /> : null}
      <ProjectSidebar
        runtimeStatus={runtimeStatus}
        projects={projects}
        activeProject={activeProject}
        selectedConversationId={activeConversation?.id ?? null}
        onProjectSelected={selectProject}
        onCreateProject={openProjectCreator}
        onConversationSelected={(conversation) => {
          setActiveConversation(conversation);
          setShowProjectSettings(false);
          setShowAppSettings(false);
          setShowWallet(false);
        }}
        onWalletSelected={() => {
          setShowWallet(true);
          setShowAppSettings(false);
          setShowProjectSettings(false);
        }}
        onAppSettingsSelected={() => {
          setShowAppSettings(true);
          setShowWallet(false);
          setShowProjectSettings(false);
        }}
        onBack={returnToWorkspace}
        backLabel={backLabel}
        settingsSelected={showProjectSettings || showAppSettings || showWallet}
        walletSelected={showWallet}
        appSettingsSelected={showAppSettings}
        historyVersion={conversationHistoryVersion}
        projectPending={projectPending}
        activeChatResponse={activeChatResponse}
      />

      <main className="workspace">
        <header className="workspace-header">
          <span>{showWallet
            ? "Wallet"
            : showAppSettings
            ? "Minutus settings"
            : showProjectSettings
            ? `${activeProject?.name ?? "Project"} settings`
            : activeConversation?.title ?? activeProject?.name ?? (modelReady ? "Projects" : "Local workspace")}</span>
          <span className="workspace-header-actions">
            {!showAppSettings && !showWallet ? (
              <button className="header-model-button" type="button" onClick={showProjectSettings ? returnToWorkspace : () => {
                setShowProjectSettings(true);
                setShowAppSettings(false);
                setShowWallet(false);
              }} disabled={activeProject === null} aria-label={showProjectSettings ? backLabel : undefined}>{showProjectSettings ? "‹ Back" : "Project settings"}</button>
            ) : null}
            <span className="privacy-label">On-device</span>
          </span>
        </header>

        {workspaceReady && activeProject ? (
          <ChatWorkspace
            projectId={activeProject.id}
            projectName={activeProject.name}
            conversation={activeConversation}
            models={models}
            defaultModel={modelPreferences.defaultModel}
            onConversationChanged={updateActiveConversation}
            onHistoryChanged={updateConversationHistory}
            onGenerationActivityChanged={updateGenerationActivity}
            hidden={showWallet || showAppSettings || showProjectSettings}
          />
        ) : null}

        {showWallet ? (
          <WalletWorkspace wallet={wallet} onChange={setWallet} />
        ) : showAppSettings ? (
          <section className="model-management" aria-labelledby="model-management-title">
            <header className="model-management-header">
              <p className="eyebrow">Application preferences</p>
              <h1 id="model-management-title">Minutus settings</h1>
              <p>Manage network endpoints, web search, and the local models available across every project.</p>
            </header>
            <div className="model-management-subheading"><p className="eyebrow">Local models</p><h2>Available models</h2></div>
            <div className="model-management-list">
              {orderedModels.map((candidate) => {
                const isDefault = modelPreferences.defaultModel?.modelId === candidate.id
                  && modelPreferences.defaultModel?.modelVersion === candidate.version;
                return (
                  <article className="managed-model-card" key={`${candidate.id}@${candidate.version}`}>
                    <div className="managed-model-heading">
                      <div className="managed-model-identity"><strong>{candidate.displayName}</strong>{candidate.recommended ? <em>Recommended</em> : null}</div>
                      <span className={candidate.installed ? "model-install-state setting-status configured" : "model-install-state setting-status"}>
                        {isDefault ? "Default · " : ""}{modelAvailabilityLabel(candidate)}
                      </span>
                    </div>
                    <div className="managed-model-body">
                      <p>{candidate.capabilities.join(" · ")}</p>
                      <div className="model-specs">
                        <span>{formatSize(candidate.bundleDownloadBytes)} complete bundle</span>
                        {!candidate.providerAvailable ? <span className="model-runtime-warning">Local runtime needs attention</span> : null}
                      </div>
                    </div>
                    <Suspense fallback={<p className="phase-note">Loading module options…</p>}>
                    <ModelFileActions settings model={candidate} disabled={downloadState === "downloading" || activeChatResponse !== null} onChanged={refreshModels}>
                      {!candidate.installed && !candidate.externalFiles ? (
                        <button className="secondary-action" type="button"
                          disabled={downloadState === "downloading"}
                          onClick={() => void installModel(candidate)}>
                          {downloadState === "downloading" && selectedModelId === candidate.id ? "Installing…" : "Download"}
                        </button>
                      ) : null}
                    </ModelFileActions>
                    </Suspense>
                  </article>
                );
              })}
            </div>
            {downloadState === "downloading" ? (
              <div className="download-progress" aria-live="polite">
                <div className="progress-copy"><strong>{phaseLabel(downloadPhase, progress?.resumed ?? false)}</strong><span>{progress ? `${percentage.toFixed(1)}%` : "Connecting…"}</span></div>
                <progress max={100} value={progress ? percentage : undefined} />
                <button className="secondary-action cancel-action" type="button" onClick={cancelDownload} disabled={cancelRequested}>{cancelRequested ? "Pausing…" : "Cancel and keep progress"}</button>
              </div>
            ) : null}
            {modelPreferenceError ? <p className="download-error" role="alert">{modelPreferenceError}</p> : null}
            {downloadError ? <p className="download-error" role="alert">{downloadError}</p> : null}
            <SpeechModelSettings />
            <section className="app-settings-section" aria-labelledby="default-model-title">
              <div className="app-settings-section-heading">
                <div><p className="eyebrow">Local models</p><h2 id="default-model-title">Default model</h2></div>
              </div>
              <div className="default-model-setting">
                <label htmlFor="default-model-select">Model for new projects and chats</label>
                <select
                  id="default-model-select"
                  value={modelPreferences.defaultModel ? `${modelPreferences.defaultModel.modelId}@${modelPreferences.defaultModel.modelVersion}` : ""}
                  onChange={(event) => {
                    const candidate = models.find((item) => `${item.id}@${item.version}` === event.target.value);
                    if (candidate) void chooseDefaultModel(candidate);
                  }}
                >
                  {modelPreferences.defaultModel === null ? <option value="">Choose an available model</option> : null}
                  {modelPreferences.defaultModel && !models.some((candidate) => candidate.installed && candidate.id === modelPreferences.defaultModel?.modelId && candidate.version === modelPreferences.defaultModel.modelVersion) ? (
                    <option disabled value={`${modelPreferences.defaultModel.modelId}@${modelPreferences.defaultModel.modelVersion}`}>Saved model · Unavailable</option>
                  ) : null}
                  {models.filter((candidate) => candidate.installed && candidate.chatSelectable).map((candidate) => (
                    <option key={`${candidate.id}@${candidate.version}`} value={`${candidate.id}@${candidate.version}`}>
                      {candidate.displayName}{candidate.recommended ? " · Recommended" : ""}
                    </option>
                  ))}
                </select>
              </div>
            </section>
            <WebSearchSettings />
            <EvmRpcSettingsPanel />
            <ReleaseUpdateSettings />
            <ThemeSettings {...theme} />
          </section>
        ) : modelReady && activeProject === null ? (
          <section className="empty-state project-onboarding" aria-labelledby="project-welcome-title">
            <span className="hero-mark" aria-hidden="true"><img className="brand-symbol" src="/brand/minutus-mark.svg" alt="" /></span>
            <p className="eyebrow">Your local workspace</p>
            <h1 id="project-welcome-title">Create a project to get started.</h1>
            <p className="welcome-copy">Keep related chats together under a shared theme. Folder access can be added later when the project needs it.</p>
            <button className="primary-action" type="button" onClick={openProjectCreator} disabled={projectPending}>
              Create project
            </button>
          </section>
        ) : workspaceReady && activeProject && showProjectSettings ? (
          <ProjectSettings project={activeProject} models={models} defaultModel={modelPreferences.defaultModel} />
        ) : workspaceReady && activeProject ? null : (
          <section className="empty-state" aria-labelledby="welcome-title">
          <span className="hero-mark" aria-hidden="true">
            <img className="brand-symbol" src="/brand/minutus-mark.svg" alt="" />
          </span>
          <p className="eyebrow">Private by default</p>
          <h1 id="welcome-title">Set up your local model.</h1>

          {model ? (
            <div className="model-card">
              <div className="onboarding-model-select">
                <label htmlFor="onboarding-model-select">Local model{moduleSetupActive || model.externalFiles ? <> <span className="model-module-requirement" data-required="true">Required</span></> : null}</label>
                <div className="onboarding-model-select-control">
                  <select
                    id="onboarding-model-select"
                    value={model.id}
                    disabled={downloadState === "downloading" || moduleOperationBusy}
                    onChange={(event) => {
                      const candidate = models.find((item) => item.id === event.target.value);
                      if (!candidate) return;
                      setModuleSetupActive(false);
                      setSelectedModelId(candidate.id);
                      setDownloadState(candidate.installed ? "installed" : "idle");
                      setDownloadError(null);
                      setProgress(null);
                    }}
                  >
                    {orderedModels.map((candidate) => (
                      <option key={candidate.id} value={candidate.id}>
                        {candidate.displayName}{candidate.recommended ? " · Recommended" : ""}
                      </option>
                    ))}
                  </select>
                  <svg className="onboarding-model-chevron" viewBox="0 0 16 16" fill="none" aria-hidden="true" focusable="false">
                    <path d="m4 6 4 4 4-4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                  </svg>
                </div>
                {model.registrationState !== "notInstalled" ? (
                  <span className={model.installed ? "setting-status configured" : "setting-status"} role="status">
                    {modelAvailabilityLabel(model)}
                  </span>
                ) : null}
              </div>
              <div className="model-details">
                <div className="model-specs">
                  <span>{formatSize(model.bundleDownloadBytes)} complete bundle</span>
                  <span>{formatSize(model.recommendedMemoryBytes)} recommended base memory</span>
                  {!model.providerAvailable ? <span className="model-runtime-warning">Local runtime needs attention</span> : null}
                </div>
              </div>

              {model.companions.length > 0 && !moduleSetupActive && !model.externalFiles ? (
                <div className="companion-options" aria-label="Included model modules">
                  <strong>Included modules</strong>
                  {model.companions.map((companion) => {
                    const title = companion.kind === "visionProjector" ? "Vision projector" : "Faster generation draft";
                    const description = companion.kind === "visionProjector"
                      ? "Adds image understanding"
                      : "Enables speculative decoding";
                    return (
                      <div className="companion-option" key={companion.artifactId}>
                        <span>
                          <strong>{title}</strong>
                          <small>{description} · {formatSize(companion.downloadBytes)} download · loaded only when needed</small>
                        </span>
                        {companion.installed ? <em>Available</em> : null}
                      </div>
                    );
                  })}
                  <p className="companion-summary">Downloading the complete bundle installs every module. Runtime memory is used only for modules needed by the active request.</p>
                </div>
              ) : null}

              {downloadState === "downloading" ? (
                <div className="download-progress" aria-live="polite">
                  <div className="progress-copy">
                    <strong>{phaseLabel(downloadPhase, progress?.resumed ?? false)}</strong>
                    <span>{progress ? `${percentage.toFixed(1)}%` : "Connecting…"}</span>
                  </div>
                  <progress
                    aria-label={phaseLabel(downloadPhase, progress?.resumed ?? false)}
                    max={100}
                    value={progress ? percentage : undefined}
                  />
                  {progress ? (
                    <div className="transfer-details">
                      <span>{formatSize(progress.aggregateDownloadedBytes)} of {formatSize(progress.aggregateTotalBytes)}</span>
                      <span>{formatSize(progress.aggregateRemainingBytes)} remaining</span>
                      {transferMetrics && progress.aggregateRemainingBytes > 0 ? (
                        <>
                          <span>{formatRate(transferMetrics.bytesPerSecond)}</span>
                          <span>{formatDuration(transferMetrics.etaSeconds)} remaining</span>
                        </>
                      ) : null}
                    </div>
                  ) : (
                    <p className="phase-note">Checking disk space and retained download data…</p>
                  )}
                  {downloadPhase === "verifying" ? (
                    <p className="phase-note">
                      Checking {progress?.companionKind ? "this companion" : "the primary model"} can take several minutes. Keep Minutus open.
                    </p>
                  ) : null}
                  {downloadPhase === "installing" ? (
                    <p className="phase-note">Verification passed. Registering the complete bundle atomically…</p>
                  ) : null}
                  {downloadPhase === "preparing" || downloadPhase === "downloading" ? (
                    <button
                      className="secondary-action cancel-action"
                      type="button"
                      onClick={cancelDownload}
                      disabled={cancelRequested}
                    >
                      {cancelRequested ? "Pausing…" : "Cancel and keep progress"}
                    </button>
                  ) : null}
                </div>
              ) : !model.externalFiles && !moduleSetupActive ? (
                <button
                  className="primary-action"
                  type="button"
                  onClick={() => void installModel()}
                  disabled={moduleOperationBusy || (model.installed && !companionsNeedInstall) || !progressListenerReady}
                >
                  {model.installed && !companionsNeedInstall
                    ? "Installed"
                    : !progressListenerReady
                      ? "Preparing progress updates…"
                    : downloadState === "failed"
                      ? "Retry download"
                    : "Download"}
                </button>
              ) : null}

              <p className="phase-note">{model.capabilities.join(" · ")}</p>

              {downloadError && <p className="download-error" role="alert">{downloadError}</p>}

              <Suspense fallback={<p className="phase-note">Loading module options…</p>}>
              <ModelFileActions key={`${model.id}@${model.version}`} model={model} setupActive={moduleSetupActive} disabled={downloadState === "downloading" || activeChatResponse !== null} onChanged={refreshModels} onSetupStarted={() => setModuleSetupActive(true)} onSetupFinished={finishModuleSetup} onBusyChange={setModuleOperationBusy} />
              </Suspense>
            </div>
          ) : (
            <p className="welcome-copy">
              {runtimeStatus === "preview"
                ? "Connect the native application to view the model catalogue."
                : "Loading the verified model catalogue…"}
            </p>
          )}
          </section>
        )}
      </main>
      {projectCreatorOpen ? (
        <CreateProjectDialog
          pending={projectPending}
          error={projectError}
          onCancel={() => {
            setProjectCreatorOpen(false);
            setProjectError(null);
          }}
          onCreate={createProject}
        />
      ) : null}
    </div>
  );
}
