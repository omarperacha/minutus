import type { EvmNetwork } from "../generated/ipc";
import type { NetworkBalance } from "./wallet/useWalletBalances";
import { TokenIcon } from "./wallet/WalletIcon";

export function EvmBalancesPanel({ balances, loading, onSend }: { balances: NetworkBalance[]; loading: boolean; onSend: (symbol: string, network: EvmNetwork) => void }) {
  if (loading) return <div className="wallet-loading" role="status">Loading your tokens…</div>;
  return <div className="wallet-token-list">
    {balances.map(({ network, name, response }) => <div key={network}>
      {!response.snapshot ? <p className="wallet-notice wallet-error" role="status">{name}: {response.errorMessage ?? "Balances unavailable. Try refreshing."}</p> : <>
        {response.freshness === "stale" ? <p className="wallet-notice" role="status">{name} · Last known balances from {new Date(response.snapshot.observedAt).toLocaleString()}. {response.errorMessage ?? "Refresh failed."}</p> : null}
        {response.snapshot.assets.map((asset) => <details className="wallet-token" key={asset.symbol}>
          <summary>
            <TokenIcon symbol={asset.symbol} />
            <span className="wallet-token-name"><strong>{asset.symbol === "ETH" ? "Ethereum" : asset.symbol === "USDC" ? "USD Coin" : asset.symbol}</strong><small><i className={`wallet-network-dot ${network.startsWith("base") ? "base" : "ethereum"}`} />{name}{response.snapshot?.isTestnet ? " · Testnet" : ""}</small></span>
            <span className="wallet-token-amount"><strong>{asset.formattedAmount}</strong><small>{asset.symbol}</small></span>
          </summary>
          <div className="wallet-token-detail">
            <dl><div><dt>Network</dt><dd>{name}</dd></div><div><dt>Updated</dt><dd>{new Date(response.snapshot!.observedAt).toLocaleString()}</dd></div>{asset.tokenContract ? <div><dt>Contract</dt><dd><code>{asset.tokenContract}</code></dd></div> : <div><dt>Asset</dt><dd>Native ETH</dd></div>}</dl>
            <button type="button" className="secondary-action" onClick={() => onSend(asset.symbol, network)}>Send {asset.symbol}</button>
          </div>
        </details>)}
      </>}
    </div>)}
    <p className="wallet-footnote">ETH and Circle-issued USDC · Select a token for details</p>
  </div>;
}
