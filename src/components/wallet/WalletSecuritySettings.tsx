import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import type { PublicWalletState } from "../../generated/ipc";
import { walletError } from "./networks";

export function WalletSecuritySettings({ onChange }: { onChange: (wallet: PublicWalletState) => void }) {
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  async function run(command: string, label: string) {
    if (pending) return;
    setPending(label); setError(null);
    try { onChange(await invoke<PublicWalletState>(command)); }
    catch (failure) { setError(walletError(failure, "The wallet operation could not be completed.")); }
    finally {
      try { onChange(await invoke<PublicWalletState>("wallet_state")); } catch { /* Keep the operation error if state is unavailable. */ }
      setPending(null);
    }
  }
  return <div aria-label="Wallet security">
    <p className="wallet-notice">One app password protects all your accounts. Each recovery phrase and imported private key needs its own backup.</p>
    {[
      ["change_wallet_password", "Change app password", "Changes the password for every account and locks the vault."],
      ["remove_wallet_account", "Remove selected account", "Keeps other accounts. Recovery requires this account’s own backup."],
      ["delete_wallet_vault", "Delete entire app vault", "Removes every local account after native confirmation and password authentication."],
      ["reset_wallet", "Forgot password? Recover with a phrase", "Replaces the entire vault with one recovered account. Other accounts need separate backups."],
      ["reset_wallet_private_key", "Forgot password? Recover with a private key", "Replaces the entire vault with one imported account. Other accounts need separate backups."],
    ].map(([command, title, description]) => <button className="wallet-settings-link" type="button" key={command} disabled={pending !== null} onClick={() => void run(command, `${title}…`)}>
      <span><strong>{title}</strong><small>{description}</small></span><span aria-hidden="true">→</span>
    </button>)}
    {pending ? <p className="wallet-notice" role="status">{pending}</p> : null}
    {error ? <p className="wallet-notice wallet-error" role="alert">{error}</p> : null}
  </div>;
}
