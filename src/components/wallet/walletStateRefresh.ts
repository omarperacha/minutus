import type { PublicWalletState } from "../../generated/ipc";

type WalletState = PublicWalletState | null;
type Publish = (next: WalletState | ((current: WalletState) => WalletState)) => void;

/** Order public-state reads around revocation; this cache never grants authority. */
export function createWalletStateRefresh(read: () => Promise<PublicWalletState>, publish: Publish) {
  let generation = 0;
  let disposed = false;
  async function refresh() {
    if (disposed) return;
    const request = ++generation;
    try {
      const next = await read();
      if (!disposed && request === generation) publish(next);
    } catch { /* Keep a revoked cache locked if storage or IPC is unavailable. */ }
  }
  return {
    refresh,
    apply(next: WalletState) {
      if (disposed) return;
      // An action result may have been sent before a later system lock. Treat
      // an unlocked result as a refresh hint, never as permission to undo it.
      if (next?.availability === "ready") { void refresh(); return; }
      generation += 1;
      publish(next);
    },
    invalidate() {
      if (disposed) return;
      generation += 1;
      publish((current) => current?.walletId ? { ...current, address: null, availability: "locked" } : current);
      void refresh();
    },
    dispose() { disposed = true; generation += 1; },
  };
}
