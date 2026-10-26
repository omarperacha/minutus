import assert from "node:assert/strict";
import test from "node:test";
import { projectTranscript } from "./transcript.ts";

const text = (text) => ({ type: "text", text });
const activity = (id, status = "completed") => ({
  type: "activity", activity: { id, kind: "fileRead", title: "Read source", detail: null, status },
});
const call = (callId) => ({ type: "toolCall", request: { callId, name: "filesystem.inspect_image", arguments: {} } });
const message = (id, role, ...parts) => ({ id, role, parts, conversationId: "conversation", createdAt: "2026-10-15T10:00:00Z" });

test("tool rounds become one response with only the final answer outside Activity", () => {
  const messages = [
    message("user", "user", text("Transcribe the image")),
    message("discovery", "assistant", text("I'll find the source."), activity("find"), call("find")),
    message("discovery-result", "tool", { type: "toolResult", result: { callId: "find", outcome: { type: "completed", content: [] } } }),
    message("inspection", "assistant", text("I'll inspect it."), activity("image"), call("image")),
    message("image-result", "tool", { type: "toolResult", result: { callId: "image", outcome: { type: "completed", content: [{ type: "asset", assetId: "source" }] } } }),
    message("write", "assistant", activity("write"), call("write")),
    message("write-result", "tool", { type: "filesystemWriteRecord", writeCallId: "write", changes: [] }),
    message("final", "assistant", activity("model-pass-0"), text("Done.")),
  ];
  const original = structuredClone(messages);
  const entries = projectTranscript(messages);
  assert.equal(entries.length, 2);
  const response = entries[1];
  assert.equal(response.answer.id, "final");
  assert.deepEqual(response.answer.parts.filter((p) => p.type === "text"), [text("Done.")]);
  assert.deepEqual(response.timeline.filter((p) => p.type === "commentary").map((p) => p.text), ["I'll find the source.", "I'll inspect it."]);
  assert.deepEqual(response.timeline.filter((p) => p.type === "image").map((p) => p.assetId), ["source"]);
  assert.deepEqual(response.activities.map((p) => p.id), ["find", "image", "write", "model-pass-0"]);
  assert.ok(response.messageIds.includes("write")); // Existing diff/undo lookup still finds its owning message.
  assert.ok(response.messageIds.includes("write-result"));
  assert.deepEqual(messages, original); // Native inference history is never rewritten.
});

test("partial or cancelled work never promotes commentary to a final answer or crosses user turns", () => {
  const user = message("first-user", "user", text("First"), { type: "asset", assetId: "attached" });
  const entries = projectTranscript([
    user,
    message("partial", "assistant", text("I'll write it."), activity("write"), call("write")),
    message("next-user", "user", text("Second")),
    message("answer", "assistant", text("Second answer")),
  ]);
  assert.equal(entries.length, 4);
  assert.equal(entries[0].message, user);
  assert.equal(entries[1].answer, null);
  assert.equal(entries[1].timeline[0].type, "commentary");
  assert.equal(entries[3].answer.id, "answer");
  assert.deepEqual(entries[3].activities, []);
});

test("activity updates deduplicate within a response and pass IDs remain separate across responses", () => {
  const entries = projectTranscript([
    message("u1", "user", text("First")),
    message("round", "assistant", activity("work", "pending"), call("work")),
    message("a1", "assistant", activity("work"), activity("model-pass-0"), text("First answer")),
    message("u2", "user", text("Second")),
    message("a2", "assistant", activity("model-pass-0"), text("Second answer")),
  ]);
  assert.equal(entries[1].activities.length, 2);
  assert.equal(entries[1].activities[0].status, "completed");
  assert.equal(entries[1].timeline.filter((p) => p.type === "activity" && p.id === "work").length, 1);
  assert.equal(entries[3].activities.length, 1);
});

test("private reasoning stays inside Activity and consecutive direct answers remain distinct", () => {
  const entries = projectTranscript([
    message("u", "user", text("Question")),
    message("a1", "assistant", { type: "reasoning", text: "Question\nPrivate reasoning" }, text("Answer")),
    message("a2", "assistant", text("Additional answer"), { type: "asset", assetId: "output" }),
  ]);
  assert.equal(entries.length, 3);
  assert.deepEqual(entries[1].timeline.map((p) => p.text), ["Private reasoning"]);
  assert.equal(entries[2].answer.id, "a2");
  assert.deepEqual(entries[2].timeline, []);
  assert.equal(entries[2].answer.parts[1].assetId, "output");
});
