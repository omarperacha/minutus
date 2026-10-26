import type { ChatEffort, CatalogueModel, ModelSelection } from "../../generated/ipc";

export type CapacityChoice = "small" | "default" | "large" | "extraLarge" | "max";
export const EXTRA_LARGE_CONTEXT_TOKENS = 131_072;

export function capacityForModel(capacity: CapacityChoice, modelLimit: number): CapacityChoice {
  return capacity === "max" && modelLimit <= EXTRA_LARGE_CONTEXT_TOKENS ? "extraLarge" : capacity;
}

type ComposerSettingsProps = {
  models: CatalogueModel[];
  model: ModelSelection | null;
  capacity: CapacityChoice;
  effort: ChatEffort | null;
  disabled: boolean;
  loadingModel: boolean;
  onModelChange: (model: ModelSelection) => void;
  onCapacityChange: (capacity: CapacityChoice) => void;
  onEffortChange: (effort: ChatEffort | null) => void;
};

export function ComposerSettings({
  models, model, capacity, effort, disabled, loadingModel,
  onModelChange, onCapacityChange, onEffortChange,
}: ComposerSettingsProps) {
  const installed = models.filter((candidate) => candidate.installed && candidate.chatSelectable);
  const selected = models.find((candidate) => candidate.id === model?.modelId && candidate.version === model.modelVersion);
  const maxLabel = selected ? `${Math.round(selected.contextLimitTokens / 1024)}k` : "";
  const visibleCapacity = capacityForModel(capacity, selected?.contextLimitTokens ?? EXTRA_LARGE_CONTEXT_TOKENS);
  return (
    <div className="composer-settings" aria-label="Settings for the next response">
      <label className="composer-setting composer-model-setting">
        <span>Model</span>
        <select
          aria-label="Model for the next response"
          value={model ? `${model.modelId}@${model.modelVersion}` : ""}
          disabled={disabled || loadingModel || installed.length === 0}
          onChange={(event) => {
            const chosen = installed.find((candidate) => `${candidate.id}@${candidate.version}` === event.target.value);
            if (chosen) onModelChange({ modelId: chosen.id, modelVersion: chosen.version });
          }}
        >
          {model === null ? <option value="">{loadingModel ? "Loading…" : "Select model"}</option> : null}
          {model && !selected?.installed ? <option disabled value={`${model.modelId}@${model.modelVersion}`}>{selected?.displayName ?? "Saved model"} · Unavailable</option> : null}
          {installed.map((candidate) => (
            <option key={`${candidate.id}@${candidate.version}`} value={`${candidate.id}@${candidate.version}`}>
              {candidate.displayName}
            </option>
          ))}
        </select>
      </label>
      <label className="composer-setting">
        <span>Capacity</span>
        <select
          aria-label="Context capacity for the next response"
          value={visibleCapacity}
          disabled={disabled || loadingModel || !selected}
          onChange={(event) => onCapacityChange(event.target.value as CapacityChoice)}
          title="Total room for instructions, history, sources and response. Larger capacities reserve more memory; changing capacity may require processing the prompt again."
        >
          <option value="small">Small · 16k</option>
          <option value="default">Default · 32k</option>
          <option value="large">Large · 64k</option>
          <option value="extraLarge">Extra large · 128k</option>
          {selected && selected.contextLimitTokens > EXTRA_LARGE_CONTEXT_TOKENS
            ? <option value="max">Max · {maxLabel}</option> : null}
        </select>
      </label>
      <label className="composer-setting">
        <span>Effort</span>
        <select
          aria-label="Reasoning effort for the next response"
          value={effort ?? "auto"}
          disabled={disabled}
          onChange={(event) => onEffortChange(event.target.value === "auto" ? null : event.target.value as ChatEffort)}
          title="Sets model reasoning guidance and the per-round output allowance. Thinking shares that allowance with answers and tool calls."
        >
          <option value="auto">Auto</option>
          <option value="quick">Quick</option>
          <option value="balanced">Balanced</option>
          <option value="deep">Deep</option>
        </select>
      </label>
    </div>
  );
}
