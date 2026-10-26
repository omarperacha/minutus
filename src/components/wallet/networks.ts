import type { EvmNetwork } from "../../generated/ipc";

export const walletNetworks: { id: EvmNetwork; name: string; testnet: boolean }[] = [
  { id: "baseMainnet", name: "Base", testnet: false },
  { id: "ethereumMainnet", name: "Ethereum", testnet: false },
  { id: "baseSepolia", name: "Base Sepolia", testnet: true },
  { id: "ethereumSepolia", name: "Ethereum Sepolia", testnet: true },
];

export type NetworkFilter = EvmNetwork | "all";

export function filteredNetworks(filter: NetworkFilter) {
  return walletNetworks.filter((network) => filter === "all" ? !network.testnet : network.id === filter);
}

export function shortAddress(address: string) {
  return `${address.slice(0, 6)}…${address.slice(-4)}`;
}

export function walletError(error: unknown, fallback: string) {
  return typeof error === "object" && error !== null && "message" in error ? String(error.message) : fallback;
}
