import assert from "node:assert/strict";
import test from "node:test";
import {
  emptyStreamOutput, hasFinalMessage, isActive, matchesGeneration,
  nextStreamPhase, reduceStreamOutput, visibleStreamText,
} from "./streamOutput.ts";

function preview(text, round = 0) {
  return { type: "textPreviewDelta", requestId: "request", sequence: 1, round, text };
}

test("preview grows before any terminal event and final replaces rather than appends", () => {
  let output = reduceStreamOutput(emptyStreamOutput(), preview("Hello "));
  assert.equal(visibleStreamText(output), "Hello ");
  assert.equal(output.text, "");
  assert.equal(output.finalMessageId, null);
  output = reduceStreamOutput(output, preview("🌍"));
  assert.equal(visibleStreamText(output), "Hello 🌍");
  output = reduceStreamOutput(output, {
    type: "final", requestId: "request", sequence: 3, messageId: "message", text: "Hello 🌍!",
  });
  assert.equal(visibleStreamText(output), "Hello 🌍!");
  assert.equal(output.previewText, "");
  assert.equal(output.finalMessageId, "message");
});

test("late tool calls clear the draft without duplicating progress as answer text", () => {
  let output = reduceStreamOutput(emptyStreamOutput(), preview("I'll read the source"));
  output = reduceStreamOutput(output, { type: "textPreviewReset", round: 0 });
  output = reduceStreamOutput(output, { type: "reasoningDelta", text: "I'll read the source" });
  assert.equal(visibleStreamText(output), "");
  assert.equal(output.finalMessageId, null);
});

test("new rounds discard rejected drafts and ignore stale round previews and resets", () => {
  let output = reduceStreamOutput(emptyStreamOutput(), preview("Premature answer"));
  output = reduceStreamOutput(output, { type: "textPreviewReset", round: 1 });
  assert.equal(visibleStreamText(output), "");
  output = reduceStreamOutput(output, preview("Correct answer", 1));
  output = reduceStreamOutput(output, preview("stale text", 0));
  output = reduceStreamOutput(output, { type: "textPreviewReset", round: 0 });
  assert.equal(visibleStreamText(output), "Correct answer");
});

test("cancellation and host errors never leave a provisional answer committed", () => {
  for (const type of ["cancelled", "error"]) {
    const output = reduceStreamOutput(
      reduceStreamOutput(emptyStreamOutput(), preview("Unverified answer")), { type },
    );
    assert.equal(visibleStreamText(output), "");
    assert.equal(output.finalMessageId, null);
  }
});

test("Glimmer's existing direct text stream still grows and commits once", () => {
  let output = reduceStreamOutput(emptyStreamOutput(), { type: "textDelta", text: "Hello" });
  output = reduceStreamOutput(output, { type: "textDelta", text: "!" });
  assert.equal(visibleStreamText(output), "Hello!");
  output = reduceStreamOutput(output, { type: "final", text: "Hello!", messageId: "final" });
  assert.equal(visibleStreamText(output), "Hello!");
});

test("late history from an earlier request cannot clear a newer request in the same conversation", () => {
  const current = { requestId: "new", conversationId: "conversation" };
  assert.equal(matchesGeneration(current, "old", "conversation"), false);
  assert.equal(matchesGeneration(current, "new", "other"), false);
  assert.equal(matchesGeneration(current, "new", "conversation"), true);
  assert.equal(matchesGeneration(null, "old", "conversation"), false);
});

test("settled text stays visible until its exact persisted message arrives", () => {
  const output = reduceStreamOutput(emptyStreamOutput(), {
    type: "final", text: "Completed", messageId: "final-message",
  });
  assert.equal(hasFinalMessage([{ id: "user-message" }], output.finalMessageId), false);
  assert.equal(visibleStreamText(output), "Completed");
  assert.equal(hasFinalMessage([{ id: "final-message" }], output.finalMessageId), true);
  assert.equal(hasFinalMessage([{ id: "final-message" }], null), false);
});

test("final delivery stops generation immediately without waiting for history IO", () => {
  for (const initial of ["preparing", "streaming", "stopping"]) {
    const phase = nextStreamPhase(initial, { type: "final" });
    assert.equal(phase, "settled");
    assert.equal(isActive(phase), false);
  }
});

test("late in-flight deltas cannot undo the stopping state", () => {
  for (const type of ["status", "textPreviewDelta", "textPreviewReset", "textDelta", "reasoningDelta", "activity", "toolCall"]) {
    assert.equal(nextStreamPhase("stopping", { type }), "stopping");
  }
  assert.equal(nextStreamPhase("stopping", { type: "cancelled" }), "cancelled");
  assert.equal(nextStreamPhase("streaming", { type: "error" }), "error");
});

test("late events cannot reopen a terminal generation", () => {
  for (const phase of ["settled", "cancelled", "error"]) {
    for (const type of ["textPreviewDelta", "reasoningDelta", "activity", "final"]) {
      assert.equal(nextStreamPhase(phase, { type }), phase);
    }
  }
});
