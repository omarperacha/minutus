import type { GenerationEvent } from "../../generated/ipc";

// Development-only browser timing. Entries contain lifecycle categories and
// sequence numbers, never prompt text, tool arguments or model output.
const receivedAt = new Map<string, number>();
const contentSeen = new Set<string>();

export function recordSendStart(): number {
  if (!import.meta.env.DEV) return 0;
  const started = performance.now();
  window.requestAnimationFrame(() => {
    performance.measure("minutus:feedback:send-to-first-paint", {
      start: started,
      end: performance.now(),
    });
  });
  return started;
}

export function recordSendAccepted(started: number): void {
  if (!import.meta.env.DEV || started === 0) return;
  performance.measure("minutus:feedback:send-to-accepted", {
    start: started,
    end: performance.now(),
  });
}

export function recordFeedbackReceipt(event: GenerationEvent): void {
  if (!import.meta.env.DEV) return;
  const requestId = event.requestId;
  if (!requestId) return;
  if (event.type === "status" && event.phase === "processingContext") contentSeen.delete(requestId);
  if (event.type === "final" || event.type === "error" || event.type === "cancelled") {
    contentSeen.delete(requestId);
  }
  if (event.type !== "status" && event.type !== "activity"
    && event.type !== "final" && event.type !== "error" && event.type !== "cancelled"
    && event.type !== "textDelta" && event.type !== "textPreviewDelta") return;
  if (event.type === "textDelta" || event.type === "textPreviewDelta") {
    if (contentSeen.has(requestId)) return;
    contentSeen.add(requestId);
  }
  receivedAt.set(`${requestId}:${event.sequence}`, performance.now());
  if (receivedAt.size > 256) receivedAt.delete(receivedAt.keys().next().value!);
  performance.mark("minutus:feedback:received", {
    detail: {
      type: event.type,
      sequence: event.sequence,
      phase: event.type === "status" ? event.phase : undefined,
    },
  });
}

export function recordFeedbackPaint(requestId: string, sequence: number): void {
  if (!import.meta.env.DEV) return;
  const key = `${requestId}:${sequence}`;
  const start = receivedAt.get(key);
  if (start === undefined) return;
  receivedAt.delete(key);
  performance.measure("minutus:feedback:receipt-to-paint", {
    start,
    end: performance.now(),
    detail: { sequence },
  });
}
