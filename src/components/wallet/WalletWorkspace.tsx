import { useEffect, useRef, useState } from "react";
import type { EvmNetwork, PublicWalletState } from "../../generated/ipc";
import { WalletLifecyclePanel } from "../WalletLifecycle";
import { EvmBalancesPanel } from "../EvmBalances";
import { EvmTransactionsPanel } from "../EvmTransactions";
import { EvmNativeTransferPanel } from "../EvmNativeTransfer";
import { EvmTokenTransferPanel } from "../EvmTokenTransfer";
import { EvmAuthorizationDecoderPanel } from "../EvmAuthorizationDecoder";
import { EvmRpcSettingsPanel } from "../EvmRpcSettings";
import { shortAddress, walletNetworks, type NetworkFilter } from "./networks";
import { useWalletBalances } from "./useWalletBalances";
import { totalBalance } from "./balanceAmounts";
import { WalletSecuritySettings } from "./WalletSecuritySettings";
import { WalletIcon } from "./WalletIcon";

type WalletProps = { wallet: PublicWalletState | null; onChange: (wallet: PublicWalletState) => void };
type View = "home" | "send" | "receive" | "settings" | "inspect";

function CopyAddress({ address, full = false }: { address: string; full?: boolean }) {
  const [status, setStatus] = useState("");
  useEffect(() => {
    if (!status) return;
    const timer = window.setTimeout(() => setStatus(""), 3000);
    return () => window.clearTimeout(timer);
  }, [status]);
  return <div className="wallet-copy-address">
    {full ? <code className="wallet-receive-address">{address}</code> : null}
    <button type="button" title={address} aria-label="Copy wallet address" onClick={() => {
      void navigator.clipboard.writeText(address).then(() => setStatus("Address copied")).catch(() => setStatus("Could not copy. Select the address to copy it manually."));
    }}>
      {full ? "Copy address" : <span>{shortAddress(address)}</span>}<WalletIcon name="copy" />
    </button>
    <span className="wallet-copy-status" role="status">{status}</span>
  </div>;
}

function WalletPortfolio({ wallet, onChange }: { wallet: PublicWalletState & { address: string }; onChange: WalletProps["onChange"] }) {
  const [filter, setFilter] = useState<NetworkFilter>("all");
  const [tab, setTab] = useState<"tokens" | "activity">("tokens");
  const [view, setView] = useState<View>("home");
  const [revision, setRevision] = useState(0);
  const [sendNetwork, setSendNetwork] = useState<EvmNetwork>("baseMainnet");
  const [sendAsset, setSendAsset] = useState("ETH");
  const balances = useWalletBalances(wallet.address, filter, revision);
  const heading = useRef<HTMLHeadingElement>(null);
  const previousView = useRef(view);
  useEffect(() => {
    if (previousView.current !== view) heading.current?.focus();
    previousView.current = view;
  }, [view]);

  function send(symbol: string, network: EvmNetwork) {
    setSendAsset(symbol);
    setSendNetwork(network);
    setView("send");
  }

  function home() {
    setView("home");
    setRevision((value) => value + 1);
  }

  const eth = balances.loading ? null : totalBalance(balances.balances, "ETH");
  const usdc = balances.loading ? null : totalBalance(balances.balances, "USDC");
  const stale = balances.balances.some((entry) => entry.response.freshness === "stale");
  const unavailable = balances.balances.some((entry) => !entry.response.snapshot);
  const selectedNetwork = walletNetworks.find((network) => network.id === filter);

  return <>
    {view === "home" ? <>
      <div className="wallet-balance-hero" aria-busy={balances.loading}>
        <h1 className="wallet-overline" ref={heading} tabIndex={-1}>Your balance{selectedNetwork ? ` on ${selectedNetwork.name}` : ""}</h1>
        <div className="wallet-total"><span>{eth ?? "—"}</span><span className="wallet-total-symbol">ETH</span></div>
        <p className="wallet-secondary-balance">{usdc ?? "—"} <span>USDC</span></p>
        <CopyAddress address={wallet.address} />
        {stale || unavailable ? <p className="wallet-balance-warning" role="status">{unavailable ? "Some balances are unavailable. Refresh to try again." : "Showing last known balances. Refresh to update."}</p> : null}
        <div className="wallet-quick-actions">
          <button type="button" onClick={() => send("ETH", filter === "all" ? "baseMainnet" : filter)}><span><WalletIcon name="send" /></span>Send</button>
          <button type="button" onClick={() => setView("receive")}><span><WalletIcon name="receive" /></span>Receive</button>
          <button type="button" onClick={() => setView("settings")}><span><WalletIcon name="settings" /></span>Settings</button>
        </div>
      </div>
      <div className="wallet-tabs" role="tablist" aria-label="Wallet content">
        {(["tokens", "activity"] as const).map((entry) => <button id={`wallet-tab-${entry}`} role="tab" aria-selected={tab === entry} aria-controls={`wallet-panel-${entry}`} tabIndex={tab === entry ? 0 : -1} key={entry} type="button" onClick={() => setTab(entry)} onKeyDown={(event) => {
          if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
            event.preventDefault();
            const next = event.key === "Home" ? "tokens" : event.key === "End" ? "activity" : tab === "tokens" ? "activity" : "tokens";
            setTab(next);
            document.getElementById(`wallet-tab-${next}`)?.focus();
          }
        }}>{entry === "tokens" ? "Tokens" : "Activity"}</button>)}
      </div>
      <div className="wallet-list-toolbar">
        <label className="wallet-network-filter"><span className={`wallet-network-dot ${filter.startsWith("base") ? "base" : "ethereum"}`} /><select aria-label="Filter by network" value={filter} onChange={(event) => setFilter(event.target.value as NetworkFilter)}>
          <option value="all">All networks</option>
          <optgroup label="Mainnets">{walletNetworks.filter((network) => !network.testnet).map((network) => <option value={network.id} key={network.id}>{network.name}</option>)}</optgroup>
          <optgroup label="Testnets">{walletNetworks.filter((network) => network.testnet).map((network) => <option value={network.id} key={network.id}>{network.name} · Testnet</option>)}</optgroup>
        </select></label>
        <button className="wallet-icon-button" aria-label={tab === "tokens" ? "Refresh balances" : "Refresh activity"} title="Refresh" type="button" disabled={balances.loading} onClick={() => setRevision((value) => value + 1)}><WalletIcon name="refresh" /></button>
      </div>
      {selectedNetwork?.testnet ? <p className="wallet-notice wallet-testnet">Testnet · {selectedNetwork.name} assets have no financial value.</p> : null}
      <div id={`wallet-panel-${tab}`} role="tabpanel" aria-labelledby={`wallet-tab-${tab}`} tabIndex={0}>
        {tab === "tokens" ? <EvmBalancesPanel balances={balances.balances} loading={balances.loading} onSend={send} /> : <EvmTransactionsPanel address={wallet.address} filter={filter} revision={revision} />}
      </div>
    </> : <div className="wallet-flow">
      <header className="wallet-flow-heading"><button className="wallet-icon-button" type="button" aria-label="Back to wallet" onClick={home}><WalletIcon name="back" /></button><h1 ref={heading} tabIndex={-1}>{view === "send" ? "Send" : view === "receive" ? "Receive" : view === "inspect" ? "Inspect authorization" : "Wallet settings"}</h1></header>
      {view === "receive" ? <div className="wallet-receive">
        <span className="wallet-welcome-icon"><WalletIcon name="receive" /></span>
        <h2>Your receiving address</h2><p>Receive ETH or USDC on Base or Ethereum at this address.</p>
        <CopyAddress address={wallet.address} full />
        <div className="wallet-receive-networks"><span><i className="wallet-network-dot base" />Base</span><span><i className="wallet-network-dot ethereum" />Ethereum</span></div>
        <p className="wallet-footnote">Choose the same network in the sending wallet. Your address is also the same on the supported Sepolia testnets.</p>
      </div> : null}
      {view === "send" ? <>
        {wallet.availability !== "ready" ? <p className="wallet-notice">Your wallet is locked. You can prepare a transfer, then unlock above to approve it.</p> : null}
        <label className="wallet-send-asset">Token<select value={sendAsset} onChange={(event) => setSendAsset(event.target.value)}><option value="ETH">Ethereum · ETH</option><option value="USDC">USD Coin · USDC</option></select></label>
        {sendAsset === "ETH" ? <EvmNativeTransferPanel key={`eth-${sendNetwork}`} wallet={wallet} initialNetwork={sendNetwork} onNetworkChange={setSendNetwork} /> : <EvmTokenTransferPanel key={`usdc-${sendNetwork}`} wallet={wallet} initialNetwork={sendNetwork} onNetworkChange={setSendNetwork} />}
      </> : null}
      {view === "settings" ? <>
        <WalletSecuritySettings onChange={onChange} />
        <button className="wallet-settings-link" type="button" onClick={() => setView("inspect")}><span><strong>Inspect authorization</strong><small>Review token approval data or a typed-data request.</small></span><span aria-hidden="true">→</span></button>
        <EvmRpcSettingsPanel />
      </> : null}
      {view === "inspect" ? <EvmAuthorizationDecoderPanel /> : null}
    </div>}
  </>;
}

export function WalletWorkspace({ wallet, onChange }: WalletProps) {
  return <section className="wallet-workspace" aria-label="Wallet"><div className="wallet-shell">
    <WalletLifecyclePanel wallet={wallet} onChange={onChange} />
    {wallet?.availability === "locked" ? <div className="wallet-flow">
      <p className="wallet-notice">Unlock to verify your accounts and show balances and receiving addresses.</p>
      <WalletSecuritySettings onChange={onChange} />
    </div> : null}
    {wallet?.availability === "ready" && wallet.address && wallet.walletId ? <WalletPortfolio key={`${wallet.walletId}:${wallet.address}`} wallet={{ ...wallet, address: wallet.address }} onChange={onChange} /> : null}
  </div></section>;
}
