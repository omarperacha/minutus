import type {
  DictationAccepted,
  DictationBinding,
  DictationEvent,
} from "../../generated/ipc";

export type DictationPhase =
  | "idle"
  | "preparing"
  | "recording"
  | "transcribing"
  | "noSpeech"
  | "error";

export type DictationState = {
  phase: DictationPhase;
  binding: DictationBinding | null;
  session: DictationAccepted | null;
  lastSequence: number;
  level: number;
  elapsedMs: number;
  durationLimitReached: boolean;
  error: string | null;
};

export type DictationTransition = {
  state: DictationState;
  transcript: string | null;
  terminal: boolean;
};

export const emptyDictationState: DictationState = {
  phase: "idle",
  binding: null,
  session: null,
  lastSequence: 0,
  level: 0,
  elapsedMs: 0,
  durationLimitReached: false,
  error: null,
};

export function bindingMatches(left: DictationBinding, right: DictationBinding): boolean {
  return left.projectId === right.projectId
    && left.conversationId === right.conversationId
    && left.composerId === right.composerId
    && left.draftRevision === right.draftRevision;
}

export function beginDictation(binding: DictationBinding): DictationState {
  return { ...emptyDictationState, phase: "preparing", binding };
}

export function acceptDictation(
  state: DictationState,
  session: DictationAccepted,
): DictationState {
  if (state.binding === null || !bindingMatches(state.binding, session.binding)) return state;
  return { ...state, session };
}

export function reduceDictationEvent(
  state: DictationState,
  envelope: DictationEvent,
): DictationTransition {
  if (state.session === null
    || envelope.sessionId !== state.session.sessionId
    || !bindingMatches(envelope.binding, state.session.binding)
    || envelope.sequence <= state.lastSequence) {
    return { state, transcript: null, terminal: false };
  }

  const sequenced = { ...state, lastSequence: envelope.sequence };
  switch (envelope.event.kind) {
    case "preparing":
      return { state: { ...sequenced, phase: "preparing" }, transcript: null, terminal: false };
    case "recording":
      return {
        state: {
          ...sequenced,
          phase: "recording",
          level: Math.max(0, Math.min(1000, envelope.event.level_millipercent)),
          elapsedMs: envelope.event.elapsed_ms,
        },
        transcript: null,
        terminal: false,
      };
    case "transcribing":
      return {
        state: {
          ...sequenced,
          phase: "transcribing",
          durationLimitReached: envelope.event.duration_limit_reached,
        },
        transcript: null,
        terminal: false,
      };
    case "completed":
      return {
        state: { ...emptyDictationState, lastSequence: envelope.sequence },
        transcript: envelope.event.text,
        terminal: true,
      };
    case "noSpeech":
      return {
        state: { ...emptyDictationState, phase: "noSpeech", lastSequence: envelope.sequence },
        transcript: null,
        terminal: true,
      };
    case "cancelled":
      return {
        state: { ...emptyDictationState, lastSequence: envelope.sequence },
        transcript: null,
        terminal: true,
      };
    case "failed":
      return {
        state: {
          ...emptyDictationState,
          phase: "error",
          lastSequence: envelope.sequence,
          error: envelope.event.message,
        },
        transcript: null,
        terminal: true,
      };
  }
}

export function replaceSelection(
  value: string,
  selectionStart: number,
  selectionEnd: number,
  transcript: string,
): { value: string; caret: number } {
  const start = Math.max(0, Math.min(value.length, Math.trunc(selectionStart)));
  const end = Math.max(start, Math.min(value.length, Math.trunc(selectionEnd)));
  return {
    value: `${value.slice(0, start)}${transcript}${value.slice(end)}`,
    caret: start + transcript.length,
  };
}

export function insertTranscriptUndoably(
  textarea: HTMLTextAreaElement,
  originalValue: string,
  selectionStart: number,
  selectionEnd: number,
  transcript: string,
): { value: string; caret: number } {
  const replacement = replaceSelection(
    originalValue,
    selectionStart,
    selectionEnd,
    transcript,
  );
  textarea.readOnly = false;
  textarea.value = originalValue;
  textarea.focus({ preventScroll: true });
  textarea.setSelectionRange(selectionStart, selectionEnd);

  // `insertText` retains WebKit's native undo transaction. `setRangeText` is
  // the standards-based fallback and uses the same prevalidated replacement.
  try {
    const inserted = typeof document !== "undefined"
      && typeof document.execCommand === "function"
      && document.execCommand("insertText", false, transcript);
    if (!inserted || textarea.value !== replacement.value) {
      textarea.value = originalValue;
      textarea.setSelectionRange(selectionStart, selectionEnd);
      textarea.setRangeText(transcript, selectionStart, selectionEnd, "end");
    }
  } finally {
    // A completed result is terminal. Leave the native control editable rather
    // than restoring the transcription-time lock after React schedules idle.
    textarea.readOnly = false;
  }
  return replacement;
}

export function isDictationActive(state: DictationState): boolean {
  return state.phase === "preparing"
    || state.phase === "recording"
    || state.phase === "transcribing";
}

export function shouldSubmitComposerKey(
  key: string,
  shiftKey: boolean,
  dictationActive: boolean,
): boolean {
  return key === "Enter" && !shiftKey && !dictationActive;
}
