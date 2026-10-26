import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { EvmBalanceResponse, EvmNetwork } from "../../generated/ipc";
import { filteredNetworks, walletError, type NetworkFilter } from "./networks";

export type NetworkBalance = { network: EvmNetwork; name: string; response: EvmBalanceResponse };

export function useWalletBalances(address: string, filter: NetworkFilter, revision: number) {
  const [state, setState] = useState<{ scope: string; balances: NetworkBalance[]; loading: boolean }>({ scope: "", balances: [], loading: true });
  const scope = `${address}:${filter}`;
  useEffect(() => {
    let active = true;
    setState({ scope, balances: [], loading: true });
    void Promise.all(filteredNetworks(filter).map(async (network): Promise<NetworkBalance> => {
      try {
        const response = await invoke<EvmBalanceResponse>("evm_balances", { request: { address, network: network.id } });
        return { network: network.id, name: network.name, response };
      } catch (error) {
        return { network: network.id, name: network.name, response: { freshness: "unavailable", snapshot: null, errorMessage: walletError(error, "Unable to load balances. Try refreshing.") } };
      }
    })).then((balances) => { if (active) setState({ scope, balances, loading: false }); });
    return () => { active = false; };
  }, [address, filter, revision, scope]);
  return state.scope === scope ? state : { scope, balances: [], loading: true };
}
