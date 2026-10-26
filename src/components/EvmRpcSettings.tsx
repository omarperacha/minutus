import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { EvmNetwork, EvmRpcSettings, IpcError } from "../generated/ipc";

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as IpcError).message);
  }
  return "EVM network settings could not be updated.";
}

export function EvmRpcSettingsPanel() {
  const [settings, setSettings] = useState<EvmRpcSettings | null>(null);
  const [pendingNetwork, setPendingNetwork] = useState<EvmNetwork | null>(null);
  const [confirmingRemove, setConfirmingRemove] = useState<EvmNetwork | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const loaded = await invoke<EvmRpcSettings>("evm_rpc_settings");
    setSettings(loaded);
  }

  useEffect(() => {
    let mounted = true;
    invoke<EvmRpcSettings>("evm_rpc_settings")
      .then((loaded) => { if (mounted) setSettings(loaded); })
      .catch((loadError) => { if (mounted) setError(errorMessage(loadError)); });
    return () => { mounted = false; };
  }, []);

  async function configure(network: EvmNetwork) {
    if (pendingNetwork !== null) return;
    setPendingNetwork(network);
    setConfirmingRemove(null);
    setError(null);
    try {
      const saved = await invoke<boolean>("configure_evm_rpc", { network });
      if (saved) await refresh();
    } catch (configureError) {
      setError(errorMessage(configureError));
    } finally {
      setPendingNetwork(null);
    }
  }

  async function remove(network: EvmNetwork) {
    if (pendingNetwork !== null) return;
    setPendingNetwork(network);
    setError(null);
    try {
      await invoke<boolean>("remove_evm_rpc", { network });
      await refresh();
      setConfirmingRemove(null);
    } catch (removeError) {
      setError(errorMessage(removeError));
    } finally {
      setPendingNetwork(null);
    }
  }

  return (
    <section className="app-settings-section" aria-labelledby="evm-rpc-settings-title">
      <div className="app-settings-section-heading">
        <div>
          <p className="eyebrow">Wallet networks</p>
          <h2 id="evm-rpc-settings-title">EVM RPC connections</h2>
        </div>
        <span className="setting-status configured">HTTPS only</span>
      </div>

      <div className="evm-rpc-settings-body">
        <p>Minutus uses credential-free defaults for every supported network. You can replace any endpoint with a production provider or your own node.</p>
        <div className="evm-network-list" aria-busy={settings === null}>
          {settings === null ? <p className="settings-loading">Loading EVM networks…</p> : settings.networks.map((network) => (
            <article className="evm-network-card" data-testnet={network.isTestnet} key={network.network}>
              <div className="evm-network-heading">
                <div>
                  <span className={network.isTestnet ? "network-kind testnet" : "network-kind"}>{network.isTestnet ? "TESTNET" : "MAINNET"}</span>
                  <h3>{network.displayName}</h3>
                  <p>{network.caip2Id} · Chain ID {network.chainId} · {network.nativeCurrencySymbol}</p>
                </div>
                <span className={network.customRpcConfigured ? "setting-status configured" : "setting-status"}>
                  {network.customRpcConfigured ? "Custom RPC" : "Default RPC"}
                </span>
              </div>
              <div className="evm-default-endpoint">
                <span>Default endpoint</span>
                <code>{network.defaultRpcUrl}</code>
              </div>
              <div className="evm-network-actions">
                <button className="secondary-action" type="button" disabled={pendingNetwork !== null} onClick={() => void configure(network.network)}>
                  {pendingNetwork === network.network ? "Updating…" : network.customRpcConfigured ? "Replace custom URL" : "Set custom URL"}
                </button>
                {network.customRpcConfigured && confirmingRemove !== network.network ? (
                  <button className="text-action danger-action" type="button" disabled={pendingNetwork !== null} onClick={() => setConfirmingRemove(network.network)}>Use default RPC</button>
                ) : null}
                {network.customRpcConfigured && confirmingRemove === network.network ? (
                  <span className="inline-confirmation">
                    <span>Remove the custom URL from Keychain?</span>
                    <button type="button" disabled={pendingNetwork !== null} onClick={() => void remove(network.network)}>Remove</button>
                    <button type="button" disabled={pendingNetwork !== null} onClick={() => setConfirmingRemove(null)}>Cancel</button>
                  </span>
                ) : null}
              </div>
            </article>
          ))}
        </div>
        <p className="settings-privacy-note">Custom URLs are collected by a native secure prompt and stored in macOS Keychain because provider URLs can contain API credentials. Minutus verifies <code>eth_chainId</code> before transaction-sensitive operations and never silently switches networks.</p>
      </div>
      {error ? <p className="download-error" role="alert">{error}</p> : null}
    </section>
  );
}
