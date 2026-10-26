import type { EvmTransactionSimulation } from "../generated/ipc";

function stateLabel(state: EvmTransactionSimulation["state"]): string {
  if (state === "succeeded") return "SIMULATED";
  return state.toUpperCase();
}

export function EvmSimulationResult({ simulation }: { simulation: EvmTransactionSimulation }) {
  return (
    <section className="evm-simulation" data-state={simulation.state} aria-label="Transaction simulation">
      <header>
        <div><p className="eyebrow">Exact-candidate simulation</p><strong>{simulation.summary}</strong></div>
        <span>{stateLabel(simulation.state)}</span>
      </header>
      {simulation.revertReason ? <p className="evm-simulation-revert" role="alert"><strong>Revert</strong><span>{simulation.revertReason}</span></p> : null}
      {simulation.state === "unavailable" || simulation.state === "inconclusive" ? <p className="evm-simulation-caveat">This result is not a successful simulation and must not be treated as approval evidence.</p> : null}
      {simulation.effects.length > 0 ? <div className="evm-simulation-effects">{simulation.effects.map((effect, index) => <p key={`${effect.kind}-${index}`} data-comparison={effect.comparison}><strong>{effect.kind === "nativeTransfer" ? "Native transfer" : "Token transfer"} · {effect.comparison.toUpperCase()}</strong><span>{effect.summary}</span></p>)}</div> : null}
      <dl className="evm-simulation-binding">
        <div><dt>Candidate digest</dt><dd><code>{simulation.candidateDigest}</code></dd></div>
        <div><dt>Bound context</dt><dd>Chain {simulation.chainId} · Block {simulation.blockNumber}<code>{simulation.blockHash}</code></dd></div>
      </dl>
    </section>
  );
}
