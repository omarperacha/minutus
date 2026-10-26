import { invoke } from "@tauri-apps/api/core";
import { FormEvent, useState } from "react";
import type {
  EvmAuthorizationDecodeRequest,
  EvmAuthorizationPayloadKind,
  EvmDecodedAuthorization,
  EvmNetwork,
  IpcError,
} from "../generated/ipc";

const USDC_CONTRACTS: Record<EvmNetwork, string> = {
  baseMainnet: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
  baseSepolia: "0x036cbd53842c5426634e7929541ec2318f3dcf7e",
  ethereumMainnet: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
  ethereumSepolia: "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238",
};

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) return String((error as IpcError).message);
  return "The authorization payload could not be decoded.";
}

function title(kind: EvmDecodedAuthorization["kind"]): string {
  if (kind === "approval") return "ERC-20 approval";
  if (kind === "permit") return "EIP-2612 Permit signature";
  if (kind === "unknownCalldata") return "Unknown contract calldata";
  return "Unknown typed data";
}

export function EvmAuthorizationDecoderPanel() {
  const [network, setNetwork] = useState<EvmNetwork>("baseMainnet");
  const [payloadKind, setPayloadKind] = useState<EvmAuthorizationPayloadKind>("approvalCalldata");
  const [owner, setOwner] = useState("");
  const [tokenContract, setTokenContract] = useState(USDC_CONTRACTS.baseMainnet);
  const [payload, setPayload] = useState("");
  const [decoded, setDecoded] = useState<EvmDecodedAuthorization | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function decode(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null); setDecoded(null);
    const request: EvmAuthorizationDecodeRequest = {
      network,
      payloadKind,
      owner: owner.trim() || null,
      tokenContract: payloadKind === "approvalCalldata" ? tokenContract.trim() || null : null,
      payload,
    };
    try { setDecoded(await invoke<EvmDecodedAuthorization>("decode_evm_authorization", { request })); }
    catch (decodeError) { setError(errorMessage(decodeError)); }
  }

  return (
    <section className="app-settings-section" aria-labelledby="evm-authorization-title">
      <div className="app-settings-section-heading">
        <div><p className="eyebrow">Authorization inspection</p><h2 id="evm-authorization-title">Decode approvals and typed data</h2></div>
        <span className="setting-status">Read only</span>
      </div>
      <div className="app-settings-section-body">
        <form className="evm-authorization-form" onSubmit={(event) => void decode(event)}>
        <p>Inspect raw approval calldata or a narrowly supported EIP-2612 Permit before any signing surface exists. Unknown content stays explicitly unknown.</p>
        <div className="evm-transfer-fields">
          <label><span>Payload type</span><select value={payloadKind} onChange={(event) => { setPayloadKind(event.target.value as EvmAuthorizationPayloadKind); setDecoded(null); }}><option value="approvalCalldata">ERC-20 approval calldata</option><option value="typedData">EIP-712 typed data</option></select></label>
          <label><span>Network</span><select value={network} onChange={(event) => { const next = event.target.value as EvmNetwork; setNetwork(next); setTokenContract(USDC_CONTRACTS[next]); setDecoded(null); }}><option value="baseMainnet">Base mainnet</option><option value="baseSepolia">Base Sepolia · TESTNET</option><option value="ethereumMainnet">Ethereum mainnet</option><option value="ethereumSepolia">Ethereum Sepolia · TESTNET</option></select></label>
          <label><span>Expected owner (optional)</span><input placeholder="0x…" value={owner} onChange={(event) => setOwner(event.target.value)} spellCheck={false} /></label>
          {payloadKind === "approvalCalldata" ? <label><span>Token contract</span><input value={tokenContract} onChange={(event) => setTokenContract(event.target.value)} spellCheck={false} /></label> : null}
        </div>
        <label className="evm-authorization-payload"><span>{payloadKind === "approvalCalldata" ? "Raw calldata" : "Raw typed-data JSON"}</span><textarea rows={8} value={payload} onChange={(event) => setPayload(event.target.value)} spellCheck={false} placeholder={payloadKind === "approvalCalldata" ? "0x095ea7b3…" : '{"types": …, "primaryType": "Permit", …}'} /></label>
        <button className="secondary-action" type="submit" disabled={payload === "" || (payloadKind === "approvalCalldata" && tokenContract.trim() === "")}>Decode authorization</button>
        </form>
        {network === "baseSepolia" || network === "ethereumSepolia" ? <p className="evm-testnet-notice"><strong>TESTNET</strong> This payload is interpreted only for the selected Sepolia network.</p> : null}
        <div aria-live="polite">
        {error ? <p className="download-error" role="alert">{error}</p> : null}
        {decoded ? (
          <article className="evm-authorization-result" data-status={decoded.status} data-testnet={decoded.isTestnet}>
            <header><div><span className={decoded.isTestnet ? "network-kind testnet" : "network-kind"}>{decoded.isTestnet ? "TESTNET" : "MAINNET"}</span><strong>{title(decoded.kind)}</strong><span>{decoded.networkName}</span></div><span className="authorization-status">{decoded.status.toUpperCase()}</span></header>
            {decoded.warnings.length > 0 ? <div className="authorization-warnings" aria-label="Authorization warnings">{decoded.warnings.map((warning, index) => <p key={`${warning.code}-${index}`} data-severity={warning.severity}><strong>{warning.code.replaceAll(/([A-Z])/g, " $1").toUpperCase()}</strong><span>{warning.message}</span></p>)}</div> : <p className="authorization-clear">No decoder warnings.</p>}
            {decoded.fields.length > 0 ? <dl className="evm-transfer-details">{decoded.fields.map((field) => <div key={field.label}><dt>{field.label}</dt><dd><code>{field.value}</code></dd></div>)}</dl> : null}
            <details className="authorization-raw"><summary>Inspect exact raw payload</summary><pre>{decoded.rawPayload}</pre></details>
            <p className="evm-preparation-note">Decoded for inspection only. This result cannot approve, sign, or broadcast anything.</p>
          </article>
        ) : null}
        </div>
      </div>
    </section>
  );
}
