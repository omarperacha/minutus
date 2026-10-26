import type { Message, TaskActivity } from "../../generated/ipc";
import type { TaskActivityTimelineItem } from "./activityTimeline";

export type TranscriptResponse = {
  type: "response";
  id: string;
  messageIds: string[];
  answer: Message | null;
  activities: TaskActivity[];
  timeline: TaskActivityTimelineItem[];
};

export type TranscriptEntry =
  | { type: "user"; id: string; message: Message }
  | TranscriptResponse;

// Persistence retains native assistant/tool exchanges for inference replay.
// Presentation groups them until an answer or the next user turn, without
// rewriting history or treating tool-round commentary as a final answer.
export function projectTranscript(messages: readonly Message[]): TranscriptEntry[] {
  const entries: TranscriptEntry[] = [];
  let response: TranscriptResponse | null = null;
  let prompt = "";
  for (const message of messages) {
    if (message.role === "user") {
      entries.push({ type: "user", id: message.id, message });
      prompt = message.parts.flatMap((part) => part.type === "text" ? [part.text] : []).join("").trim();
      response = null;
      continue;
    }
    if (!response) {
      response = { type: "response", id: message.id, messageIds: [], answer: null, activities: [], timeline: [] };
      entries.push(response);
    }
    response.messageIds.push(message.id);
    const toolRound = message.parts.some((part) => part.type === "toolCall");
    for (const [index, part] of message.parts.entries()) {
      const id = `${message.id}:${index}`;
      if (part.type === "activity") {
        const activityIndex = response.activities.findIndex((item) => item.id === part.activity.id);
        const item: TaskActivityTimelineItem = { type: "activity", id: part.activity.id, activity: part.activity };
        if (activityIndex < 0) response.activities.push(part.activity);
        else response.activities[activityIndex] = part.activity;
        const timelineIndex = response.timeline.findIndex((item) => item.type === "activity" && item.id === part.activity.id);
        if (timelineIndex < 0) response.timeline.push(item);
        else response.timeline[timelineIndex] = item;
      } else if (message.role === "assistant" && part.type === "reasoning") {
        const text = prompt && part.text.startsWith(prompt) ? part.text.slice(prompt.length).trimStart() : part.text;
        if (text) response.timeline.push({ type: "reasoning", id, text });
      } else if (message.role === "assistant" && toolRound && part.type === "text" && part.text) {
        response.timeline.push({ type: "commentary", id, text: part.text });
      } else if (part.type === "toolResult" && part.result.outcome.type === "completed") {
        for (const [contentIndex, content] of part.result.outcome.content.entries()) {
          if (content.type === "asset") {
            response.timeline.push({ type: "image", id: `${id}:${contentIndex}`, assetId: content.assetId });
          }
        }
      } else if (message.role === "assistant" && toolRound && part.type === "asset") {
        response.timeline.push({ type: "image", id, assetId: part.assetId });
      }
    }
    if (message.role === "assistant" && !toolRound) {
      response.answer = message;
      response = null;
    }
  }
  return entries;
}
