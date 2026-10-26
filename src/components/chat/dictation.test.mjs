import assert from "node:assert/strict";
import test from "node:test";

import {
  acceptDictation,
  beginDictation,
  insertTranscriptUndoably,
  reduceDictationEvent,
  replaceSelection,
  shouldSubmitComposerKey,
} from "./dictation.ts";

const binding = {
  projectId: "project",
  conversationId: null,
  composerId: "composer",
  draftRevision: 4,
};
const session = { sessionId: "session", binding };

test("transcript replaces the captured selection without changing surrounding text", () => {
  assert.deepEqual(replaceSelection("hello brave world", 6, 11, "quiet"), {
    value: "hello quiet world",
    caret: 11,
  });
});

test("native insertion uses one selection edit and unlocks the resulting draft", () => {
  const textarea = {
    value: "",
    readOnly: true,
    selectionStart: 0,
    selectionEnd: 0,
    focus() {},
    setSelectionRange(start, end) {
      this.selectionStart = start;
      this.selectionEnd = end;
    },
    setRangeText(text, start, end) {
      this.value = `${this.value.slice(0, start)}${text}${this.value.slice(end)}`;
      this.selectionStart = start + text.length;
      this.selectionEnd = this.selectionStart;
    },
  };
  const result = insertTranscriptUndoably(textarea, "alpha omega", 6, 6, "beta ");
  assert.deepEqual(result, { value: "alpha beta omega", caret: 11 });
  assert.equal(textarea.value, result.value);
  assert.equal(textarea.selectionStart, result.caret);
  assert.equal(textarea.readOnly, false);
});

test("completed transcript is accepted once for the exact session and draft", () => {
  let state = acceptDictation(beginDictation(binding), session);
  const completed = {
    sessionId: "session",
    binding,
    sequence: 2,
    event: { kind: "completed", text: "dictated", language: "en" },
  };
  const first = reduceDictationEvent(state, completed);
  assert.equal(first.transcript, "dictated");
  assert.equal(first.terminal, true);
  state = first.state;
  const duplicate = reduceDictationEvent(state, completed);
  assert.equal(duplicate.transcript, null);
  assert.equal(duplicate.terminal, false);
});

test("stale and cross-composer results cannot modify the draft", () => {
  const state = acceptDictation(beginDictation(binding), session);
  const event = {
    sessionId: "session",
    binding: { ...binding, draftRevision: 5 },
    sequence: 1,
    event: { kind: "completed", text: "wrong", language: "en" },
  };
  assert.equal(reduceDictationEvent(state, event).transcript, null);
  assert.equal(reduceDictationEvent(state, { ...event, sessionId: "other" }).transcript, null);
  assert.equal(acceptDictation(beginDictation(binding), {
    ...session,
    binding: { ...binding, composerId: "other" },
  }).session, null);
});

test("no-speech and failure terminals never produce insertion text", () => {
  const active = acceptDictation(beginDictation(binding), session);
  const noSpeech = reduceDictationEvent(active, {
    sessionId: "session",
    binding,
    sequence: 1,
    event: { kind: "noSpeech" },
  });
  assert.equal(noSpeech.transcript, null);
  assert.equal(noSpeech.state.phase, "noSpeech");
  const failed = reduceDictationEvent(active, {
    sessionId: "session",
    binding,
    sequence: 1,
    event: { kind: "failed", code: "recognitionFailed", message: "Try again." },
  });
  assert.equal(failed.transcript, null);
  assert.equal(failed.state.error, "Try again.");
});

test("Enter cannot submit while dictation owns the composer", () => {
  assert.equal(shouldSubmitComposerKey("Enter", false, false), true);
  assert.equal(shouldSubmitComposerKey("Enter", false, true), false);
  assert.equal(shouldSubmitComposerKey("Enter", true, false), false);
});
