import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import type { PublicWalletState } from "../generated/ipc";
import { shortAddress, walletError } from "./wallet/networks";
import { WalletIcon } from "./wallet/WalletIcon";

type WalletLifecycleProps = {
  wallet: PublicWalletState | null;
  onChange: (wallet: PublicWalletState) => void;
};

export function WalletLifecyclePanel({ wallet, onChange }: WalletLifecycleProps) {
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [storageError, setStorageError] = useState<string | null>(null);
  const [inventoryRevision, setInventoryRevision] = useState(0);
  const [inventory, setInventory] = useState<PublicWalletState[]>([]);
  const menu = useRef<HTMLDetailsElement>(null);
  const configured = Boolean(wallet?.walletId);
  const ready = wallet?.availability === "ready";

  useEffect(() => {
    let active = true;
    void invoke<PublicWalletState[]>("wallet_inventory")
      .then((entries) => { if (active) { setInventory(entries); setStorageError(null); } })
      .catch((failure) => { if (active) setStorageError(walletError(failure, "Stored accounts could not be loaded. The existing vault has not been reset.")); });
    return () => { active = false; };
  }, [wallet?.walletId, wallet?.availability, wallet?.address, configured, inventoryRevision]);

  useEffect(() => {
    function close(event: PointerEvent) {
      if (menu.current && !menu.current.contains(event.target as Node)) menu.current.open = false;
    }
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, []);

  async function run(command: string, label: string, walletId?: string) {
    if (pending || (command === "select_wallet" && walletId === wallet?.walletId)) return;
    setPending(label);
    setError(null);
    if (menu.current) menu.current.open = false;
    try {
      onChange(await invoke<PublicWalletState>(command, walletId ? { walletId } : undefined));
    } catch (operationError) {
      setError(walletError(operationError, "The wallet operation could not be completed."));
    } finally {
      try {
        setInventory(await invoke<PublicWalletState[]>("wallet_inventory"));
        onChange(await invoke<PublicWalletState>("wallet_state"));
      } catch { /* Preserve the operation error when storage is unavailable. */ }
      setPending(null);
    }
  }

  const accountIndex = inventory.findIndex((entry) => entry.walletId === wallet?.walletId);
  const accountName = accountIndex >= 0 ? `Account ${accountIndex + 1}` : "My account";
  const accountActions = <>
    <button type="button" disabled={pending !== null} onClick={() => void run("create_wallet", "Creating wallet…")}>Create wallet</button>
    <button type="button" disabled={pending !== null} onClick={() => void run("restore_wallet", "Restoring wallet…")}>Restore recovery phrase</button>
    <button type="button" disabled={pending !== null} onClick={() => void run("import_wallet_private_key", "Importing wallet…")}>Import private key</button>
  </>;

  return <div className={configured ? "wallet-account" : "wallet-welcome"}>
    {storageError ? <>
      <h1>Your wallet needs attention.</h1>
      <p className="wallet-notice wallet-error" role="alert">{storageError}</p>
      <p>Check the vault file and its permissions, then retry or reopen Minutus. The existing vault has not been reset. Chats and other features remain available.</p>
      <button type="button" onClick={() => setInventoryRevision((value) => value + 1)}>Retry loading wallet</button>
    </> : configured ? <div className="wallet-account-bar">
      <details className="wallet-account-menu" ref={menu} onKeyDown={(event) => { if (event.key === "Escape" && menu.current) { menu.current.open = false; menu.current.querySelector("summary")?.focus(); } }}>
        <summary aria-label="Select account">
          <span className="wallet-identicon" aria-hidden="true">{wallet?.address?.slice(-2).toUpperCase()}</span>
          <span><strong>{accountName}</strong><small>{wallet?.address ? shortAddress(wallet.address) : ready ? "No address" : "Unlock to verify address"}</small></span>
          <WalletIcon name="chevron" />
        </summary>
        <div className="wallet-account-dropdown">
          <p className="wallet-overline">Your accounts</p>
          {inventory.filter((entry) => entry.walletId).map((entry, index) => <button type="button" key={entry.walletId} disabled={pending !== null} aria-pressed={entry.walletId === wallet?.walletId} onClick={() => void run("select_wallet", "Switching account…", entry.walletId!)}>
            <span><strong>Account {index + 1}</strong><small>{entry.address ? shortAddress(entry.address) : ready ? "No address" : "Unlock to verify address"}</small></span>
            {entry.walletId === wallet?.walletId ? <span aria-label="Selected">✓</span> : null}
          </button>)}
          <div className="wallet-account-add">{accountActions}</div>
        </div>
      </details>
      <button className="wallet-lock-button" type="button" disabled={pending !== null} onClick={() => void run(ready ? "lock_wallet" : "unlock_wallet", ready ? "Locking…" : "Unlocking…")}>
        <WalletIcon name={ready ? "unlock" : "lock"} /><span>{ready ? "Lock" : "Unlock"}</span>
      </button>
    </div> : <>
      <span className="wallet-welcome-icon"><WalletIcon name="wallet" /></span>
      <p className="wallet-overline">Your wallet, in Minutus</p>
      <h1>A home for your tokens.</h1>
      <p>Send, receive, and keep track of your ETH and USDC across Base and Ethereum.</p>
      <div className="wallet-welcome-actions">{accountActions}</div>
      <small>One app password protects every account. Passwords, recovery phrases and private keys stay in native macOS windows. Keep a separate backup for each phrase and imported key.</small>
    </>}
    {pending ? <p className="wallet-notice" role="status">{pending}</p> : null}
    {error ? <p className="wallet-notice wallet-error" role="alert">{error}</p> : null}
  </div>;
}
