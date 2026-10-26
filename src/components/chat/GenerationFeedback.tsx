import { useEffect, useState } from "react";
import type { TaskActivity } from "../../generated/ipc";
import { milestoneLabel, milestoneState } from "./feedbackProjection";

type GenerationFeedbackProps = {
  label: string;
  startedAt: number;
  round: number;
  activities: readonly TaskActivity[];
  stopping: boolean;
  canStop: boolean;
  onStop: () => void;
};

function formatElapsed(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  return `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
}

export function GenerationFeedback({ label, startedAt, round, activities, stopping, canStop, onStop }: GenerationFeedbackProps) {
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const [phaseSeconds, setPhaseSeconds] = useState(0);
  const [visibleLabel, setVisibleLabel] = useState(label);
  useEffect(() => {
    const update = () => setElapsedSeconds(Math.max(0, Math.floor((Date.now() - startedAt) / 1000)));
    update();
    const timer = window.setInterval(update, 1000);
    return () => window.clearInterval(timer);
  }, [startedAt]);

  useEffect(() => {
    if (label !== "Loading local model" && label !== "Preparing next pass") {
      setVisibleLabel(label);
      return;
    }
    const timer = window.setTimeout(() => setVisibleLabel(label), 350);
    return () => window.clearTimeout(timer);
  }, [label]);

  useEffect(() => {
    setPhaseSeconds(0);
    const phaseStartedAt = Date.now();
    const timer = window.setInterval(() => setPhaseSeconds(Math.floor((Date.now() - phaseStartedAt) / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, [visibleLabel, startedAt]);

  const statusLabel = stopping ? "Stopping" : visibleLabel;
  const needsYou = activities.some((activity) => activity.kind === "permissionDecision" && activity.status === "pending");
  return (
    <div className="generation-feedback">
      <div className="generation-now" data-state={needsYou ? "needs-you" : "working"}>
        <span className="generation-now-mark" aria-hidden="true" />
        <span className="generation-now-copy">
          <strong role="status" aria-live="polite">{statusLabel}</strong>
          {needsYou ? <small>Your approval is needed</small>
            : phaseSeconds >= 20 && !stopping ? <small>Still working on this step</small> : null}
        </span>
        {elapsedSeconds >= 4 ? <span className="generation-elapsed" aria-hidden="true">{round >= 0 ? "Pass " : ""}{formatElapsed(elapsedSeconds)}</span> : null}
        <button type="button" className="generation-stop" onClick={onStop} disabled={!canStop || stopping}>
          {stopping ? "Stopping" : "Stop"}
        </button>
      </div>
      <MilestoneTrail activities={activities} animate />
    </div>
  );
}

export function MilestoneTrail({ activities, animate = false }: { activities: readonly TaskActivity[]; animate?: boolean }) {
  const milestones = activities.filter((activity) => activity.kind !== "modelPass").slice(-4);
  if (milestones.length === 0) return null;
  return (
    <ol className="generation-milestones" aria-label="Recent work" data-animate={animate}>
      {milestones.map((activity) => (
        <li key={activity.id} data-state={milestoneState(activity)}>
          <span className="milestone-mark" aria-hidden="true" />
          <span>{milestoneLabel(activity)}</span>
        </li>
      ))}
    </ol>
  );
}
