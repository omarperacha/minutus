import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { IpcError } from "../generated/ipc";

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as IpcError).message);
  }
  return "Web search settings could not be updated.";
}

export function WebSearchSettings() {
  const [configured, setConfigured] = useState(false);
  const [pending, setPending] = useState(true);
  const [confirmingRemove, setConfirmingRemove] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let mounted = true;
    invoke<boolean>("brave_search_configured")
      .then((value) => { if (mounted) setConfigured(value); })
      .catch((loadError) => { if (mounted) setError(errorMessage(loadError)); })
      .finally(() => { if (mounted) setPending(false); });
    return () => { mounted = false; };
  }, []);

  async function configure() {
    if (pending) return;
    setPending(true);
    setError(null);
    setConfirmingRemove(false);
    try {
      const saved = await invoke<boolean>("configure_brave_search");
      if (saved) setConfigured(true);
    } catch (configureError) {
      setError(errorMessage(configureError));
    } finally {
      setPending(false);
    }
  }

  async function remove() {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await invoke<boolean>("remove_brave_search");
      setConfigured(false);
      setConfirmingRemove(false);
    } catch (removeError) {
      setError(errorMessage(removeError));
    } finally {
      setPending(false);
    }
  }

  return (
    <section className="app-settings-section" aria-labelledby="web-search-settings-title">
      <div className="app-settings-section-heading">
        <div>
          <p className="eyebrow">Web search</p>
          <h2 id="web-search-settings-title">Brave Search API</h2>
        </div>
        <span className={configured ? "setting-status configured" : "setting-status"}>
          {pending ? "Checking…" : configured ? "Configured" : "Not configured"}
        </span>
      </div>

      <div className="web-search-settings-body">
        <p>Minutus uses Brave Search to discover relevant public webpages. Search queries are sent directly from this device to Brave only after you approve the exact query. Opening a result requires a separate approval.</p>
        <div className="brave-setup-guide">
          <strong>How to get your key</strong>
          <ol>
            <li>Open the Brave Search API dashboard and create or sign in to your account.</li>
            <li>Subscribe to the Search plan. Brave requires payment details, including when monthly credits cover usage.</li>
            <li>Open <strong>API Keys</strong>, add a key named “Minutus,” and associate it with your Search plan.</li>
            <li>Copy the key, return here, and save it securely in macOS Keychain.</li>
          </ol>
          <p><a href="https://api-dashboard.search.brave.com/documentation/guides/authentication" target="_blank" rel="noreferrer">Open Brave’s API key instructions</a></p>
        </div>
        <p className="settings-privacy-note">Brave currently advertises $5 of monthly credits and a Search price of $5 per 1,000 requests. Approved searches count against your Brave account. Brave states that API queries may be retained for up to 90 days for billing and troubleshooting; queries do not pass through Flowzed.</p>
      </div>

      <div className="app-settings-actions">
        <button className="secondary-action" type="button" onClick={configure} disabled={pending}>
          {configured ? "Replace API key" : "Add API key"}
        </button>
        {configured && !confirmingRemove ? (
          <button className="text-action danger-action" type="button" onClick={() => setConfirmingRemove(true)} disabled={pending}>Remove API key</button>
        ) : null}
        {configured && confirmingRemove ? (
          <span className="inline-confirmation">
            <span>Remove the key from Keychain?</span>
            <button type="button" onClick={remove} disabled={pending}>Remove</button>
            <button type="button" onClick={() => setConfirmingRemove(false)} disabled={pending}>Cancel</button>
          </span>
        ) : null}
      </div>
      {error ? <p className="download-error" role="alert">{error}</p> : null}
    </section>
  );
}
