import type { ChatGenerationPhase, TaskActivity } from "../../generated/ipc";

export function currentWorkLabel(
  phase: ChatGenerationPhase,
  activities: readonly TaskActivity[],
  hasPreview: boolean,
  hasAnswerText: boolean,
  hasReasoning: boolean,
): string {
  if (activities.some((item) => item.kind === "permissionDecision" && item.status === "pending")) {
    return "Waiting for your decision";
  }
  if (phase === "executingTools") {
    const latest = activities.filter((item) => item.kind !== "modelPass").at(-1);
    if (latest?.status === "inProgress") return milestoneLabel(latest);
    if (latest?.status === "pending") return "Waiting for an action result";
    if (latest) return "Recording results";
    return "Checking requested actions";
  }
  if (phase === "savingResponse") return "Saving response";
  switch (phase) {
    case "startingResponse": return "Starting response";
    case "loadingLocalModel": return "Loading local model";
    case "preparingConversationContext": return "Preparing next pass";
    case "processingContext": return "Reading context and generating";
    case "thinking":
      if (hasPreview) return "Streaming draft";
      if (hasAnswerText) return "Composing response";
      return hasReasoning ? "Thinking" : "Receiving model output";
  }
}

export function milestoneLabel(activity: TaskActivity): string {
  if (activity.status !== "inProgress" && activity.status !== "pending") return activity.title;
  switch (activity.kind) {
    case "fileRead": return "Reading project files";
    case "fileWrite": return "Writing project files";
    case "search": return "Finding information";
    case "pureComputation": return "Running local computation";
    case "projectComputation": return "Computing with selected files";
    case "proposedWrite": return "Preparing a change";
    case "permissionDecision": return "Waiting for your decision";
    case "transactionPreparation": return "Preparing transaction";
    case "transactionStatus": return "Checking transaction";
    case "modelReasoning": return "Thinking";
    case "modelPass": return "Reading context and generating";
  }
}

export function milestoneState(activity: TaskActivity): "active" | "done" | "attention" | "needs-you" | "stopped" {
  if (activity.kind === "permissionDecision" && activity.status === "pending") return "needs-you";
  if (activity.status === "cancelled") return "stopped";
  if (activity.status === "failed" || activity.status === "denied" || activity.status === "partial") return "attention";
  if (activity.status === "pending" || activity.status === "inProgress") return "active";
  return "done";
}
