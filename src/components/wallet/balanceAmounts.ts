import type { NetworkBalance } from "./useWalletBalances";

// Keep base units exact even when a balance is larger than Number.MAX_SAFE_INTEGER.
export function totalBalance(balances: NetworkBalance[], symbol: string): string | null {
  const assets = balances.map(({ response }) => response.snapshot?.assets.find((asset) => asset.symbol === symbol));
  const first = assets[0];
  if (!first || assets.some((asset) => !asset || asset.decimals !== first.decimals)) return null;
  const raw = assets.reduce((sum, asset) => sum + BigInt(asset!.rawAmount), 0n);
  const digits = raw.toString().padStart(first.decimals + 1, "0");
  if (first.decimals === 0) return digits;
  const fraction = digits.slice(-first.decimals).replace(/0+$/, "");
  return `${digits.slice(0, -first.decimals)}${fraction ? `.${fraction}` : ""}`;
}
