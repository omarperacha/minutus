import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import type { IpcError, SpeechModelView } from "../generated/ipc";

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as IpcError).message);
  }
  return "Speech model setup could not be completed. Try again.";
}

function statusLabel(model: SpeechModelView): string {
  switch (model.state.kind) {
    case "missing": return "Not installed";
    case "installed": return "Installed";
    case "incompatible": return "Remove and download again";
    case "inUse": return "In use";
    case "downloading": {
      const progress = model.state.progress;
      if (!progress) return "Preparing…";
      if (progress.phase === "verifying") return "Verifying…";
      if (progress.phase === "installing") return "Installing…";
      return `${progress.resumed ? "Resuming" : "Downloading"}… ${Math.floor(progress.downloadedBytes / progress.totalBytes * 100)}%`;
    }
  }
}

export function SpeechModelSettings() {
  const [models, setModels] = useState<SpeechModelView[]>([]);
  const [pending, setPending] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const mounted = useRef(false);
  const refresh = useRef<() => Promise<void>>(async () => {});

  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let loading = false;
    async function load() {
      if (disposed || loading) return;
      loading = true;
      clearTimeout(timer);
      try {
        const next = await invoke<SpeechModelView[]>("speech_models");
        if (disposed) return;
        setModels(next);
        // Host snapshots survive navigation. Poll only while an operation owns
        // the weights; idle full-file verification runs on entry/focus/actions.
        if (next.some((model) => model.state.kind === "downloading" || model.state.kind === "inUse")) {
          timer = setTimeout(load, 500);
        }
      } catch (loadError) {
        if (!disposed) setError(errorMessage(loadError));
      } finally { loading = false; }
    }
    refresh.current = load;
    void load();
    const onFocus = () => { void load(); };
    window.addEventListener("focus", onFocus);
    return () => {
      disposed = true;
      mounted.current = false;
      clearTimeout(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, []);

  async function perform(model: SpeechModelView, operation: "install_speech_model" | "remove_speech_model") {
    if (pending) return;
    setPending(true);
    setError(null);
    setNotice(null);
    const work = invoke<void>(operation, { request: model.selection });
    // Begin progress observation while the native command remains pending.
    const timer = setInterval(() => { void refresh.current(); }, 500);
    try {
      await work;
      if (mounted.current) setNotice(operation === "install_speech_model" ? "Speech model installed." : "Speech model removed.");
    } catch (operationError) {
      if (mounted.current) {
        const cancelled = typeof operationError === "object" && operationError !== null && "code" in operationError && (operationError as IpcError).code === "cancelled";
        if (cancelled) setNotice("Download cancelled. Download again to resume the retained bytes.");
        else setError(errorMessage(operationError));
      }
    } finally {
      clearInterval(timer);
      if (mounted.current) {
        setPending(false);
        setCancelling(false);
        await refresh.current();
      }
    }
  }

  async function cancel() {
    setCancelling(true);
    try { await invoke<boolean>("cancel_speech_download"); }
    catch (cancelError) { if (mounted.current) setError(errorMessage(cancelError)); }
    finally { if (mounted.current) { setCancelling(false); await refresh.current(); } }
  }

  return (
    <section className="app-settings-section" aria-labelledby="speech-model-title">
      <div className="app-settings-section-heading">
        <div><p className="eyebrow">Speech</p><h2 id="speech-model-title">Local dictation model</h2></div>
      </div>
      <div className="app-settings-section-body">
        <p>Prepare the optional speech model on this device for microphone dictation in the chat composer.</p>
        <p className="settings-privacy-note">Installation does not activate your microphone. Transcription will run locally without an internet connection.</p>
      </div>
      {models.length === 0 && !error ? <div className="app-settings-section-body"><p role="status">Checking speech model…</p></div> : null}
      {models.map((model) => {
        const downloading = model.state.kind === "downloading";
        const progress = model.state.kind === "downloading" ? model.state.progress : null;
        return (
          <div key={`${model.selection.modelId}@${model.selection.modelVersion}`}>
            <div className="app-settings-section-body">
            <p><strong>{model.displayName}</strong> · {(model.downloadBytes / 1_000_000).toFixed(1)} MB download</p>
            <p role="status">{statusLabel(model)}</p>
            {progress ? <progress aria-label="Speech model download" max={progress.totalBytes} value={progress.downloadedBytes} /> : null}
            </div>
            <div className="app-settings-actions">
              {model.state.kind === "missing" ? <button className="secondary-action" type="button" disabled={pending} onClick={() => { void perform(model, "install_speech_model"); }}>Download speech model</button> : null}
              {downloading ? <button className="secondary-action" type="button" disabled={cancelling} onClick={() => { void cancel(); }}>{cancelling ? "Cancelling…" : "Cancel download"}</button> : null}
              {model.state.kind === "installed" || model.state.kind === "incompatible" ? <button className="text-action danger-action" type="button" disabled={pending} onClick={() => { void perform(model, "remove_speech_model"); }}>Remove speech model</button> : null}
              <a className="text-action" href={model.licenseUrl} target="_blank" rel="noreferrer">Model licence</a>
            </div>
          </div>
        );
      })}
      {notice ? <div className="app-settings-section-body"><p role="status">{notice}</p></div> : null}
      {error ? <div className="app-settings-section-body"><p className="download-error" role="alert">{error} <button type="button" onClick={() => { setError(null); void refresh.current(); }}>Retry status</button></p></div> : null}
    </section>
  );
}
