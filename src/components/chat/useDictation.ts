import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import type {
  ConversationId,
  DictationAccepted,
  DictationBinding,
  DictationCommand,
  DictationEvent,
  DictationEventsRequest,
  IpcError,
  ProjectId,
  SpeechModelRequest,
  StartDictationRequest,
} from "../../generated/ipc";
import {
  acceptDictation,
  beginDictation,
  emptyDictationState,
  insertTranscriptUndoably,
  isDictationActive,
  reduceDictationEvent,
} from "./dictation";
import type { DictationState } from "./dictation";

type SelectionSnapshot = {
  value: string;
  start: number;
  end: number;
};

type UseDictationOptions = {
  projectId: ProjectId;
  conversationId: ConversationId | null;
  draft: string;
  draftRevision: number;
  textarea: RefObject<HTMLTextAreaElement | null>;
  disabled: boolean;
  onDraftChange: (value: string) => void;
};

const POLL_DELAY_MS = 50;

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as IpcError).message);
  }
  return "Dictation is unavailable. Try again.";
}

function composerIdentity(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `composer-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

export function useDictation({
  projectId,
  conversationId,
  draft,
  draftRevision,
  textarea,
  disabled,
  onDraftChange,
}: UseDictationOptions) {
  const [state, setState] = useState<DictationState>(emptyDictationState);
  const stateRef = useRef(state);
  const mounted = useRef(true);
  const operation = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const selection = useRef<SelectionSnapshot | null>(null);
  const draftRef = useRef(draft);
  const composerId = useRef(composerIdentity());

  draftRef.current = draft;

  const store = useCallback((next: DictationState) => {
    stateRef.current = next;
    if (mounted.current) setState(next);
  }, []);

  const restoreSelection = useCallback((snapshot: SelectionSnapshot, caret?: number) => {
    requestAnimationFrame(() => {
      const element = textarea.current;
      if (!mounted.current || !element) return;
      const position = caret ?? snapshot.start;
      element.focus({ preventScroll: true });
      element.setSelectionRange(position, position);
    });
  }, [textarea]);

  const commandFor = useCallback((session: DictationAccepted): DictationCommand => ({
    sessionId: session.sessionId,
    binding: session.binding,
  }), []);

  const stopPolling = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  }, []);

  const abandon = useCallback((message: string, session?: DictationAccepted) => {
    operation.current += 1;
    stopPolling();
    const active = session ?? stateRef.current.session;
    if (active) void invoke<boolean>("cancel_dictation", { command: commandFor(active) });
    store({ ...emptyDictationState, phase: "error", error: message });
    const snapshot = selection.current;
    if (snapshot) restoreSelection(snapshot);
  }, [commandFor, restoreSelection, stopPolling, store]);

  const poll = useCallback(async function pollSession(
    epoch: number,
    session: DictationAccepted,
  ): Promise<void> {
    if (!mounted.current || operation.current !== epoch) return;
    const request: DictationEventsRequest = {
      sessionId: session.sessionId,
      binding: session.binding,
      afterSequence: stateRef.current.lastSequence,
    };
    try {
      const events = await invoke<DictationEvent[]>("dictation_events", { request });
      if (!mounted.current || operation.current !== epoch) return;
      for (const event of events) {
        const transition = reduceDictationEvent(stateRef.current, event);
        store(transition.state);
        if (transition.transcript !== null) {
          const snapshot = selection.current;
          const element = textarea.current;
          if (!snapshot || !element || draftRef.current !== snapshot.value) {
            abandon("The draft changed before dictation finished. No text was inserted.", session);
            return;
          }
          const inserted = insertTranscriptUndoably(
            element,
            snapshot.value,
            snapshot.start,
            snapshot.end,
            transition.transcript,
          );
          onDraftChange(inserted.value);
          restoreSelection(snapshot, inserted.caret);
        } else if (transition.terminal && selection.current) {
          restoreSelection(selection.current);
        }
        if (transition.terminal) {
          stopPolling();
          return;
        }
      }
      timer.current = setTimeout(() => { void pollSession(epoch, session); }, POLL_DELAY_MS);
    } catch (error) {
      if (operation.current === epoch) abandon(errorMessage(error), session);
    }
  }, [abandon, onDraftChange, restoreSelection, stopPolling, store, textarea]);

  const captureSelection = useCallback(() => {
    const element = textarea.current;
    if (!element || isDictationActive(stateRef.current)) return;
    selection.current = {
      value: draftRef.current,
      start: element.selectionStart ?? draftRef.current.length,
      end: element.selectionEnd ?? draftRef.current.length,
    };
  }, [textarea]);

  const start = useCallback(async () => {
    if (disabled || isDictationActive(stateRef.current)) return;
    const element = textarea.current;
    const snapshot = selection.current?.value === draftRef.current
      ? selection.current
      : element ? {
          value: draftRef.current,
          start: element.selectionStart ?? draftRef.current.length,
          end: element.selectionEnd ?? draftRef.current.length,
        } : null;
    if (!snapshot) return;
    selection.current = snapshot;
    const binding: DictationBinding = {
      projectId,
      conversationId,
      composerId: composerId.current,
      draftRevision,
    };
    const epoch = operation.current + 1;
    operation.current = epoch;
    store(beginDictation(binding));
    try {
      const model = await invoke<SpeechModelRequest>("speech_model_selection");
      if (!mounted.current || operation.current !== epoch) return;
      const request: StartDictationRequest = { binding, model, language: null };
      const accepted = await invoke<DictationAccepted>("start_dictation", { request });
      if (!mounted.current || operation.current !== epoch) {
        void invoke<boolean>("cancel_dictation", { command: commandFor(accepted) });
        return;
      }
      const next = acceptDictation(stateRef.current, accepted);
      if (next.session === null) {
        abandon("The dictation session did not match this composer. No text was inserted.", accepted);
        return;
      }
      store(next);
      void poll(epoch, accepted);
    } catch (error) {
      if (operation.current === epoch) abandon(errorMessage(error));
    }
  }, [abandon, commandFor, conversationId, disabled, draftRevision, poll, projectId, store, textarea]);

  const finish = useCallback(async () => {
    const session = stateRef.current.session;
    if (!session || stateRef.current.phase !== "recording") return;
    try {
      await invoke<boolean>("finish_dictation", { command: commandFor(session) });
    } catch (error) {
      abandon(errorMessage(error), session);
    }
  }, [abandon, commandFor]);

  const cancel = useCallback(() => {
    const current = stateRef.current;
    if (!isDictationActive(current)) return;
    operation.current += 1;
    stopPolling();
    if (current.session) {
      void invoke<boolean>("cancel_dictation", { command: commandFor(current.session) });
    }
    store(emptyDictationState);
    if (selection.current) restoreSelection(selection.current);
  }, [commandFor, restoreSelection, stopPolling, store]);

  const dismiss = useCallback(() => store(emptyDictationState), [store]);

  useEffect(() => {
    const handleKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || !isDictationActive(stateRef.current)) return;
      event.preventDefault();
      cancel();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [cancel]);

  const location = useRef({ projectId, conversationId });
  useEffect(() => {
    const previous = location.current;
    location.current = { projectId, conversationId };
    if (previous.projectId !== projectId || previous.conversationId !== conversationId) cancel();
  }, [cancel, conversationId, projectId]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      operation.current += 1;
      stopPolling();
      const session = stateRef.current.session;
      if (session) void invoke<boolean>("cancel_dictation", { command: commandFor(session) });
    };
  }, [commandFor, stopPolling]);

  return {
    state,
    active: isDictationActive(state),
    captureSelection,
    start,
    finish,
    cancel,
    dismiss,
  };
}
