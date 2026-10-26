import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

export function ReleaseUpdateSettings() {
  const [configured, setConfigured] = useState(false);
  const [busy, setBusy] = useState(false);
  const [version, setVersion] = useState<string | null>(null);
  const [message, setMessage] = useState("Checking update availability…");

  useEffect(() => {
    let mounted = true;
    invoke<boolean>("release_updates_configured").then((enabled) => {
      if (!mounted) return;
      setConfigured(enabled);
      setMessage(enabled ? "Updates are checked only when you choose." : "Signed updates are not available in this build.");
    }).catch(() => { if (mounted) setMessage("Update availability could not be checked."); });
    return () => { mounted = false; };
  }, []);

  async function check() {
    if (busy) return;
    setBusy(true);
    setVersion(null);
    setMessage("Checking for an update…");
    try {
      const offered = await invoke<string | null>("check_release_update");
      setVersion(offered);
      setMessage(offered ? `Version ${offered} is available. Its signature will be verified before you can save it.` : "You have the latest available version.");
    } catch {
      setMessage("The update check failed. Your installed app is unchanged.");
    } finally { setBusy(false); }
  }

  async function download() {
    if (busy || !version) return;
    setBusy(true);
    setMessage("Downloading and verifying the update…");
    try {
      const saved = await invoke<boolean>("save_verified_release_update");
      setMessage(saved ? "Verified update saved. Quit Minutus, open the disk image and replace Minutus in Applications, then reopen it." : "Save cancelled. Your installed app is unchanged.");
    } catch {
      setMessage("The update could not be verified or saved. Your installed app is unchanged. Check again to retry.");
    } finally { setBusy(false); setVersion(null); }
  }

  return <section className="app-settings-section" aria-labelledby="release-update-title">
    <div className="app-settings-section-heading"><div><p className="eyebrow">Application</p><h2 id="release-update-title">Signed updates</h2></div></div>
    <div className="app-settings-section-body release-update-body">
    <p>Checking contacts minutus.ai. Downloads are verified against the release signing key before they are saved. Installing an update keeps your local data and models.</p>
    <p role="status">{message}</p>
    </div>
    <div className="app-settings-actions">
      <button className="secondary-action" type="button" onClick={check} disabled={!configured || busy}>Check for updates</button>
      {version ? <button className="secondary-action" type="button" onClick={download} disabled={busy}>Download verified update</button> : null}
    </div>
  </section>;
}
