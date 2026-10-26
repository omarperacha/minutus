import assert from "node:assert/strict";
import test from "node:test";
import { currentWorkLabel, milestoneLabel, milestoneState } from "./feedbackProjection.ts";

const completedRead = {
  id: "read-1", kind: "fileRead", title: "Read a workspace file", detail: null, status: "completed",
};

test("each host phase has a truthful visible state before any work completes", () => {
  assert.deepEqual([
    "startingResponse", "loadingLocalModel", "preparingConversationContext",
    "processingContext", "thinking", "executingTools", "savingResponse",
  ].map((phase) => currentWorkLabel(phase, [], false, false, false)), [
    "Starting response", "Loading local model", "Preparing next pass",
    "Reading context and generating", "Receiving model output", "Checking requested actions", "Saving response",
  ]);
});

test("continuations name observed phases without claiming a separate review action", () => {
  assert.equal(currentWorkLabel("loadingLocalModel", [completedRead], false, false, false), "Loading local model");
  assert.equal(currentWorkLabel("preparingConversationContext", [completedRead], false, false, false), "Preparing next pass");
  assert.equal(currentWorkLabel("processingContext", [completedRead], false, false, false), "Reading context and generating");
  assert.equal(currentWorkLabel("thinking", [completedRead], false, false, false), "Receiving model output");
  assert.equal(currentWorkLabel("thinking", [completedRead], false, false, true), "Thinking");
  assert.equal(currentWorkLabel("thinking", [completedRead], true, false, false), "Streaming draft");
  assert.equal(currentWorkLabel("thinking", [completedRead], false, true, false), "Composing response");
  assert.equal(currentWorkLabel("executingTools", [], false, false, false), "Checking requested actions");
  assert.equal(currentWorkLabel("executingTools", [{ ...completedRead, status: "inProgress" }], false, false, false), "Reading project files");
  assert.equal(currentWorkLabel("executingTools", [completedRead], false, false, false), "Recording results");
  assert.equal(currentWorkLabel("executingTools", [completedRead], true, false, false), "Recording results");
});

test("pending human decisions remain distinct from failures", () => {
  const pending = { id: "decision", kind: "permissionDecision", title: "Permission required", status: "pending" };
  assert.equal(currentWorkLabel("executingTools", [pending], false, false, false), "Waiting for your decision");
  assert.equal(milestoneState(pending), "needs-you");
  assert.equal(milestoneState({ ...pending, status: "denied" }), "attention");
  assert.equal(milestoneState({ ...completedRead, status: "failed" }), "attention");
  assert.equal(milestoneState({ ...completedRead, status: "cancelled" }), "stopped");
});

test("the compact trail uses present-tense language until work completes", () => {
  assert.equal(milestoneLabel({ ...completedRead, status: "inProgress" }), "Reading project files");
  assert.equal(milestoneLabel(completedRead), "Read a workspace file");
  assert.equal(milestoneState(completedRead), "done");
});

test("a model pass after a verified write does not conceal the write or a later tool", () => {
  const write = { id: "write-1", kind: "fileWrite", title: "Saved table", status: "completed" };
  const pass = { id: "model-pass-6", kind: "modelPass", title: "Model pass 7", status: "inProgress" };
  assert.equal(currentWorkLabel("processingContext", [write, pass], false, false, false), "Reading context and generating");
  assert.equal(currentWorkLabel("executingTools", [write, pass], false, false, false), "Recording results");
});
