import type { DictationState } from "./dictation";

type DictationButtonProps = {
  state: DictationState;
  disabled: boolean;
  onCaptureSelection: () => void;
  onStart: () => void;
  onFinish: () => void;
};

function MicrophoneGlyph() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M6.5 11.5a5.5 5.5 0 0 0 11 0M12 17v4M9 21h6" />
    </svg>
  );
}

export function DictationButton({
  state,
  disabled,
  onCaptureSelection,
  onStart,
  onFinish,
}: DictationButtonProps) {
  const recording = state.phase === "recording";
  const transcribing = state.phase === "transcribing";
  const preparing = state.phase === "preparing";
  const label = recording ? "Finish dictation" : transcribing ? "Transcribing speech" : preparing
    ? "Preparing dictation" : "Dictate message";
  return (
    <button
      className="dictation-action"
      data-recording={recording}
      type="button"
      onPointerDown={onCaptureSelection}
      onClick={() => { if (recording) void onFinish(); else void onStart(); }}
      disabled={disabled || preparing || transcribing}
      aria-label={label}
      title={label}
    >
      <MicrophoneGlyph />
    </button>
  );
}

type DictationStatusProps = {
  state: DictationState;
  onCancel: () => void;
  onDismiss: () => void;
};

function elapsedLabel(elapsedMs: number): string {
  const totalSeconds = Math.floor(elapsedMs / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = String(totalSeconds % 60).padStart(2, "0");
  return `${minutes}:${seconds}`;
}

export function DictationStatus({ state, onCancel, onDismiss }: DictationStatusProps) {
  if (state.phase === "idle") return null;
  if (state.phase === "error") {
    return (
      <div className="dictation-error" role="alert">
        <span>{state.error}</span>
        <button type="button" onClick={onDismiss}>Dismiss</button>
      </div>
    );
  }
  if (state.phase === "noSpeech") {
    return (
      <div className="dictation-status" role="status">
        <span>No speech detected. Your draft was not changed.</span>
        <button type="button" onClick={onDismiss}>Dismiss</button>
      </div>
    );
  }
  const label = state.phase === "preparing"
    ? "Preparing microphone…"
    : state.phase === "transcribing"
      ? state.durationLimitReached ? "Recording limit reached. Transcribing…" : "Transcribing…"
      : `Recording ${elapsedLabel(state.elapsedMs)}`;
  return (
    <div className="dictation-status" role="status" aria-live="polite">
      <span>{label}</span>
      {state.phase === "recording" ? (
        <span className="dictation-meter">
          <span
            role="meter"
            aria-label="Microphone input level"
            aria-valuemin={0}
            aria-valuemax={1000}
            aria-valuenow={state.level}
            style={{ transform: `scaleX(${state.level / 1000})` }}
          />
        </span>
      ) : null}
      <button type="button" onClick={onCancel}>Cancel</button>
    </div>
  );
}
