import assert from "node:assert/strict";
import test from "node:test";
import { advanceRoundFeedback, initialRoundFeedback } from "./roundFeedback.ts";
import { emptyStreamOutput, reduceStreamOutput, visibleStreamText } from "./streamOutput.ts";

test("a new model pass resets channel labels and starts a new elapsed timer", () => {
  let state = initialRoundFeedback(100);
  state = advanceRoundFeedback(state, { type: "textPreviewReset", round: 0 }, 110);
  state = advanceRoundFeedback(state, { type: "reasoningDelta", text: "x" }, 120);
  state = advanceRoundFeedback(state, { type: "textDelta", text: "y" }, 130);
  assert.equal(state.roundHasReasoning, true);
  assert.equal(state.roundHasAnswer, true);

  state = advanceRoundFeedback(state, { type: "textPreviewReset", round: 0 }, 200);
  assert.equal(state.roundStartedAt, 110);

  state = advanceRoundFeedback(state, { type: "textPreviewReset", round: 1 }, 300);
  assert.deepEqual(state, {
    currentRound: 1,
    roundStartedAt: 300,
    roundHasReasoning: false,
    roundHasAnswer: false,
  });
  state = advanceRoundFeedback(state, { type: "textPreviewReset", round: 0 }, 400);
  assert.equal(state.currentRound, 1);
  assert.equal(state.roundStartedAt, 300);
});

test("feedback projection cannot overwrite streamed text or the final message when merged", () => {
  for (const type of ["textPreviewDelta", "textDelta"]) {
    let state = { ...emptyStreamOutput(), ...initialRoundFeedback(100) };
    for (const text of ["Hello", " world"]) {
      const event = { type, text, round: 0 };
      state = {
        ...state,
        ...reduceStreamOutput(state, event),
        ...advanceRoundFeedback(state, event, 110),
      };
    }
    assert.equal(visibleStreamText(state), "Hello world");
    assert.equal(state.roundHasAnswer, true);
    const final = { type: "final", text: "Hello world!", messageId: "saved-answer" };
    state = {
      ...state,
      ...reduceStreamOutput(state, final),
      ...advanceRoundFeedback(state, final, 120),
    };
    assert.equal(visibleStreamText(state), "Hello world!");
    assert.equal(state.finalMessageId, "saved-answer");
    assert.equal(state.previewText, "");
  }
});

test("feedback returns only owned fields even when passed complete generation state", () => {
  const state = { ...emptyStreamOutput(), ...initialRoundFeedback(100), phase: "streaming" };
  for (const type of ["status", "reasoningDelta", "textDelta", "textPreviewDelta", "final"]) {
    assert.deepEqual(Object.keys(advanceRoundFeedback(state, { type }, 110)).sort(), [
      "currentRound", "roundHasAnswer", "roundHasReasoning", "roundStartedAt",
    ]);
  }
});
