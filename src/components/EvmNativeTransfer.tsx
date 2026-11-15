import { invoke } from "@tauri-apps/api/core";
import { FormEvent, useState } from "react";
import type { EvmApprovalReviewOutcome, EvmNativeTransferCandidate, EvmNativeTransferRequest, EvmNetwork, IpcError, PublicWalletState } from "../generated/ipc";
import { EvmSimulationResult } from "./EvmSimulationResult";

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as IpcError).message);
  }
  return "The native transfer could not be prepared.";
}

export function EvmNativeTransferPanel({ wallet, initialNetwork = "baseMainnet", onNetworkChange }: { wallet: PublicWalletState | null; initialNetwork?: EvmNetwork; onNetworkChange?: (network: EvmNetwork) => void }) {
  const [network, setNetwork] = useState<EvmNetwork>(initialNetwork);
  const sender = wallet?.address ?? "";
  const [recipient, setRecipient] = useState("");
  const [amountEth, setAmountEth] = useState("");
  const [candidate, setCandidate] = useState<EvmNativeTransferCandidate | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewing, setReviewing] = useState(false);
  const [reviewOutcome, setReviewOutcome] = useState<EvmApprovalReviewOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function prepare(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending || reviewing) return;
    setPending(true);
    setError(null);
    setCandidate(null);
    setReviewOutcome(null);
    try {
      const request: EvmNativeTransferRequest = {
        network,
        sender: sender.trim(),
        recipient: recipient.trim(),
        amountEth: amountEth.trim(),
      };
      setCandidate(await invoke<EvmNativeTransferCandidate>("prepare_evm_native_transfer", { request }));
    } catch (prepareError) {
      setError(errorMessage(prepareError));
    } finally {
      setPending(false);
    }
  }

  async function review() {
    if (!candidate || reviewing || reviewOutcome) return;
    setReviewing(true);
    setError(null);
    try {
      setReviewOutcome(await invoke<EvmApprovalReviewOutcome>("review_evm_transaction", { reviewId: candidate.reviewId }));
    } catch (reviewError) {
      setError(errorMessage(reviewError));
    } finally {
      setReviewing(false);
    }
  }

  const isTestnet = network === "baseSepolia" || network === "ethereumSepolia";

  return (
    <section className="app-settings-section" aria-labelledby="evm-native-transfer-title">
      <div className="app-settings-section-heading">
        <div><p className="eyebrow">Native transfer</p><h2 id="evm-native-transfer-title">Send ETH</h2></div>
      </div>
      <form className="evm-transfer-form" onSubmit={(event) => void prepare(event)}>
        <p>Enter a recipient and amount. Review the transfer and network fee before approving in the native window.</p>
        <fieldset className="evm-transfer-fields" disabled={pending || reviewing}>
          <label><span>From</span><input placeholder="Create or restore a wallet first" value={sender} readOnly spellCheck={false} /></label>
          <label><span>Recipient</span><input placeholder="0x…" value={recipient} onChange={(event) => { setRecipient(event.target.value); setCandidate(null); setReviewOutcome(null); }} spellCheck={false} /></label>
          <label><span>Amount</span><span className="evm-amount-input"><input inputMode="decimal" placeholder="0.0" value={amountEth} onChange={(event) => { setAmountEth(event.target.value); setCandidate(null); setReviewOutcome(null); }} /><strong>ETH</strong></span></label>
          <label><span>Network</span><select value={network} onChange={(event) => { setNetwork(event.target.value as EvmNetwork); onNetworkChange?.(event.target.value as EvmNetwork); setCandidate(null); setReviewOutcome(null); }}><option value="baseMainnet">Base mainnet</option><option value="baseSepolia">Base Sepolia · TESTNET</option><option value="ethereumMainnet">Ethereum mainnet</option><option value="ethereumSepolia">Ethereum Sepolia · TESTNET</option></select></label>
        </fieldset>
        <button className="primary-action" type="submit" disabled={pending || reviewing || sender.trim() === "" || recipient.trim() === "" || amountEth.trim() === ""}>{pending ? "Preparing and simulating…" : "Continue"}</button>
      </form>
      {isTestnet ? <p className="evm-testnet-notice"><strong>TESTNET</strong> Sepolia ETH has no financial value.</p> : null}
      <div className="evm-transfer-result" aria-live="polite" aria-busy={pending}>
        {pending ? <p className="settings-loading">Verifying chain, balance, nonce, fees, and exact-block simulation…</p> : null}
        {error ? <p className="download-error" role="alert">{error}</p> : null}
        {candidate ? (
          <article className="evm-transfer-candidate" data-testnet={candidate.isTestnet}>
            <header>
              <div><span className={candidate.isTestnet ? "network-kind testnet" : "network-kind"}>{candidate.isTestnet ? "TESTNET" : "MAINNET"}</span><strong>{candidate.networkName}</strong><span>{candidate.caip2Id} · Chain ID {candidate.chainId}</span></div>
              <span className="balance-freshness">PREPARED</span>
            </header>
            <p className="evm-transfer-summary">Send <strong>{candidate.amountEth} ETH</strong> to <code>{candidate.recipient}</code></p>
            <EvmSimulationResult simulation={candidate.simulation} />
            <p className="wallet-review-fee">Initial network fee quote <strong>{candidate.maximumNetworkFeeEth} ETH</strong></p>
            <p className="evm-preparation-note">Your fee budget, including any headroom, is shown in the native review before approval.</p>
            <details className="wallet-review-details"><summary>Transaction details</summary>
            <dl className="evm-transfer-details">
              <div><dt>Sender</dt><dd><code>{candidate.sender}</code></dd></div>
              <div><dt>Recipient</dt><dd><code>{candidate.recipient}</code></dd></div>
              <div><dt>Raw value</dt><dd><code>{candidate.valueWei}</code> wei</dd></div>
              <div><dt>Pending nonce</dt><dd><code>{candidate.nonce}</code></dd></div>
              <div><dt>Gas limit</dt><dd><code>{candidate.gasLimit}</code></dd></div>
              <div><dt>Base fee / gas</dt><dd><code>{candidate.baseFeePerGasWei}</code> wei</dd></div>
              <div><dt>Max priority fee / gas</dt><dd><code>{candidate.maxPriorityFeePerGasWei}</code> wei</dd></div>
              <div><dt>Max fee / gas</dt><dd><code>{candidate.maxFeePerGasWei}</code> wei</dd></div>
              <div><dt>Max execution fee</dt><dd><code>{candidate.maximumExecutionFeeWei}</code> wei</dd></div>
              {candidate.network === "baseMainnet" || candidate.network === "baseSepolia" ? <><div><dt>Initial L1 fee quote</dt><dd><code>{candidate.maximumL1FeeWei}</code> wei</dd></div><div><dt>Operator fee</dt><dd><code>{candidate.operatorFeeWei}</code> wei</dd></div></> : null}
              <div className="evm-total-fee"><dt>Initial network fee quote</dt><dd><strong>{candidate.maximumNetworkFeeEth} ETH</strong><span><code>{candidate.maximumNetworkFeeWei}</code> wei</span></dd></div>
              <div><dt>Transaction</dt><dd>Type {candidate.transactionType} · Data <code>{candidate.data}</code></dd></div>
            </dl>
            </details>
            <p className="evm-preparation-note">This webview is preparation context only. Final approval is accepted exclusively in Minutus’s native trusted transaction window.</p>
            <button className="primary-action" type="button" onClick={() => void review()} disabled={reviewing || reviewOutcome !== null || wallet?.availability !== "ready"}>{reviewing ? "Opening trusted window…" : reviewOutcome ? "Review completed" : wallet?.availability !== "ready" ? "Unlock wallet to review" : "Review, sign and broadcast"}</button>
            {reviewOutcome ? <p className="evm-preparation-note" role="status">Native decision: <strong>{reviewOutcome.decision.toUpperCase()}</strong>. {reviewOutcome.transactionHash ? <>Broadcast as <code>{reviewOutcome.transactionHash}</code>.</> : "No transaction was signed."}</p> : null}
          </article>
        ) : null}
      </div>
    </section>
  );
}
