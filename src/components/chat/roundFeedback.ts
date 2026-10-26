import type { GenerationEvent } from "../../generated/ipc";

export type RoundFeedback = {
  currentRound: number;
  roundStartedAt: number;
  roundHasReasoning: boolean;
  roundHasAnswer: boolean;
};

export function initialRoundFeedback(startedAt: number): RoundFeedback {
  return {
    currentRound: -1,
    roundStartedAt: startedAt,
    roundHasReasoning: false,
    roundHasAnswer: false,
  };
}

export function advanceRoundFeedback(
  current: RoundFeedback,
  event: GenerationEvent,
  receivedAt: number,
): RoundFeedback {
  // Callers pass the full generation state. Project only this helper's fields:
  // spreading `current` would overwrite newly reduced text and final-message IDs.
  const feedback: RoundFeedback = {
    currentRound: current.currentRound,
    roundStartedAt: current.roundStartedAt,
    roundHasReasoning: current.roundHasReasoning,
    roundHasAnswer: current.roundHasAnswer,
  };
  if (event.type === "textPreviewReset" && event.round > current.currentRound) {
    return {
      currentRound: event.round,
      roundStartedAt: receivedAt,
      roundHasReasoning: false,
      roundHasAnswer: false,
    };
  }
  if (event.type === "reasoningDelta") return { ...feedback, roundHasReasoning: true };
  if (event.type === "textDelta" || event.type === "textPreviewDelta") {
    return { ...feedback, roundHasAnswer: true };
  }
  return feedback;
}
