import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { EvmTrackedTransaction } from "../generated/ipc";
import { filteredNetworks, shortAddress, walletError, type NetworkFilter } from "./wallet/networks";
import { WalletIcon } from "./wallet/WalletIcon";

function statusLabel(transaction: EvmTrackedTransaction): string {
  if (!transaction.statusAvailable) return "Status unavailable";
  switch (transaction.lifecycle) {
    case "pending": return "Pending";
    case "confirmed": return "Confirmed";
    case "failed": return "Failed";
    case "droppedOrReplaced": return "Dropped or replaced";
  }
}

export function EvmTransactionsPanel({ address, filter, revision }: { address: string; filter: NetworkFilter; revision: number }) {
  const [transactions, setTransactions] = useState<EvmTrackedTransaction[]>([]);
  const [pending, setPending] = useState(true);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setPending(true);
    setError(null);
    void invoke<EvmTrackedTransaction[]>("evm_transactions")
      .then((result) => { if (active) setTransactions(result); })
      .catch((failure) => { if (active) setError(walletError(failure, "Activity could not be loaded. Try refreshing.")); })
      .finally(() => { if (active) setPending(false); });
    return () => { active = false; };
  }, [address, revision]);

  const networks = filteredNetworks(filter).map((entry) => entry.id);
  const visible = transactions.filter((transaction) => transaction.sender.toLowerCase() === address.toLowerCase() && networks.includes(transaction.network))
    .sort((a, b) => b.submittedAt.localeCompare(a.submittedAt));

  return <div className="wallet-activity" aria-busy={pending}>
    {error ? <p className="wallet-notice wallet-error" role="alert">{error}</p> : null}
    {pending ? <div className="wallet-loading" role="status">Refreshing activity…</div> : null}
    {!pending && !error && visible.length === 0 ? <div className="wallet-empty"><WalletIcon name="activity" /><h3>No activity yet</h3><p>Transactions sent from Minutus will appear here.</p></div> : null}
    {visible.map((transaction) => <details className="wallet-transaction" key={transaction.transactionHash} data-state={transaction.statusAvailable ? transaction.lifecycle : "unavailable"}>
      <summary>
        <span className="wallet-transaction-icon"><WalletIcon name="send" /></span>
        <span className="wallet-transaction-name"><strong>Transaction</strong><small>{transaction.networkName}{transaction.isTestnet ? " · Testnet" : ""} · {new Date(transaction.submittedAt).toLocaleDateString(undefined, { month: "short", day: "numeric" })}</small></span>
        <span className="wallet-transaction-label"><strong>{statusLabel(transaction)}</strong><small>{shortAddress(transaction.transactionHash)}</small></span>
      </summary>
      <div className="wallet-transaction-detail">
        <dl>
          <div><dt>Transaction hash</dt><dd><code>{transaction.transactionHash}</code></dd></div>
          <div><dt>From</dt><dd><code>{transaction.sender}</code></dd></div>
          <div><dt>Submitted</dt><dd>{new Date(transaction.submittedAt).toLocaleString()}</dd></div>
          <div><dt>Last checked</dt><dd>{new Date(transaction.updatedAt).toLocaleString()}</dd></div>
          <div><dt>Nonce</dt><dd>{transaction.nonce}</dd></div>
          {transaction.blockNumber ? <div><dt>Block</dt><dd>{transaction.blockNumber}</dd></div> : null}
        </dl>
        {transaction.statusError ? <p className="wallet-notice">Status refresh unavailable: {transaction.statusError}. Last known state: {transaction.lifecycle}.</p> : null}
        {transaction.lifecycle === "failed" && transaction.statusAvailable ? <p className="wallet-notice wallet-error">This transaction was included in a block but failed.</p> : null}
        {transaction.lifecycle === "droppedOrReplaced" && transaction.statusAvailable ? <p className="wallet-notice">The transaction is absent and the sender nonce has advanced.</p> : null}
        <a className="evm-explorer-link" href={transaction.explorerUrl} target="_blank" rel="noreferrer">View on block explorer ↗</a>
      </div>
    </details>)}
    {visible.length > 0 ? <p className="wallet-footnote">Transactions submitted through Minutus. Incoming transfers and activity from other wallets are not included.</p> : null}
  </div>;
}
