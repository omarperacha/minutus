import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import type { PublicWalletState } from "../../generated/ipc";
import { createWalletStateRefresh } from "./walletStateRefresh";

export function useWalletState(visible: boolean) {
  const [wallet, setWallet] = useState<PublicWalletState | null>(null);
  const current = useRef<ReturnType<typeof createWalletStateRefresh> | null>(null);
  const update = useCallback((next: PublicWalletState) => {
    if (current.current) current.current.apply(next);
    else setWallet(next);
  }, []);

  useEffect(() => {
    const observer = createWalletStateRefresh(() => invoke<PublicWalletState>("wallet_state"), setWallet);
    current.current = observer;
    let active = true;
    let stopListening: (() => void) | undefined;
    // No event payload can unlock the wallet. Revocation clamps the cache, then
    // a fresh IPC read supplies the authoritative public state.
    void listen("wallet-locked", () => observer.invalidate()).then((stop) => {
      if (!active) { stop(); return; }
      stopListening = stop;
      void observer.refresh(); // Reconcile any event missed before subscription.
    }).catch(() => undefined);
    const refresh = () => { void observer.refresh(); };
    const visibilityChanged = () => { if (document.visibilityState === "visible") refresh(); };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", visibilityChanged);
    if (visible) refresh();
    const interval = visible ? window.setInterval(refresh, 15_000) : undefined;
    return () => {
      active = false;
      observer.dispose();
      current.current = null;
      stopListening?.();
      if (interval !== undefined) window.clearInterval(interval);
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", visibilityChanged);
    };
  }, [visible]);
  return [wallet, update] as const;
}
