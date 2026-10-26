import type { GenerationEvent } from "../../generated/ipc";

export type GenerationPhase = "preparing" | "streaming" | "stopping" | "settled" | "cancelled" | "error";

export function isActive(phase: GenerationPhase): boolean {
  return phase === "preparing" || phase === "streaming" || phase === "stopping";
}

export function nextStreamPhase(phase: GenerationPhase, event: GenerationEvent): GenerationPhase {
  if (!isActive(phase)) return phase;
  switch (event.type) {
    case "final": return "settled";
    case "cancelled": return "cancelled";
    case "error": return "error";
    case "textPreviewDelta":
    case "textPreviewReset":
    case "textDelta":
    case "reasoningDelta":
    case "activity":
    case "toolCall": return phase === "stopping" ? "stopping" : "streaming";
    default: return phase;
  }
}

export type StreamOutput = {
  text: string;
  previewText: string;
  previewRound: number;
  finalMessageId: string | null;
};

export function emptyStreamOutput(): StreamOutput {
  return { text: "", previewText: "", previewRound: -1, finalMessageId: null };
}

// Display-only drafts never become persisted messages or task outcomes here.
export function reduceStreamOutput(current: StreamOutput, event: GenerationEvent): StreamOutput {
  switch (event.type) {
    case "textPreviewDelta":
      if (event.round < current.previewRound) return current;
      return {
        ...current,
        previewRound: event.round,
        previewText: (event.round === current.previewRound ? current.previewText : "") + event.text,
      };
    case "textPreviewReset":
      if (event.round < current.previewRound) return current;
      return { ...current, previewRound: event.round, previewText: "" };
    case "textDelta":
      return { ...current, text: current.text + event.text };
    case "final":
      return { ...current, text: event.text, previewText: "", finalMessageId: event.messageId };
    case "cancelled":
    case "error":
      return { ...current, previewText: "" };
    default:
      return current;
  }
}

export function visibleStreamText(output: StreamOutput): string {
  return output.text + output.previewText;
}

export function matchesGeneration(
  current: { requestId: string | null; conversationId: string | null } | null,
  requestId: string,
  conversationId: string,
): boolean {
  return current?.requestId === requestId && current.conversationId === conversationId;
}

export function hasFinalMessage(
  messages: readonly { id: string }[],
  finalMessageId: string | null,
): boolean {
  return finalMessageId !== null && messages.some((message) => message.id === finalMessageId);
}
