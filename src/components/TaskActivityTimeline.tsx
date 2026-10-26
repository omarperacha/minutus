import { useEffect, useId, useRef, useState } from "react";
import type { TaskActivity, TaskActivityKind, TaskActivityStatus } from "../generated/ipc";
import { ManagedImagePreview } from "./ManagedImagePreview";
import type { TaskActivityTimelineItem } from "./chat/activityTimeline";

type TaskActivityTimelineProps = {
  reasoning: string;
  activities: TaskActivity[];
  items?: TaskActivityTimelineItem[];
  active: boolean;
};

const BOTTOM_THRESHOLD_PX = 24;

const activityLabels: Record<TaskActivityKind, string> = {
  modelReasoning: "Model reasoning",
  modelPass: "Model pass",
  fileRead: "File read",
  fileWrite: "File write",
  search: "Search",
  proposedWrite: "Proposed write",
  permissionDecision: "Permission decision",
  transactionPreparation: "Transaction preparation",
  transactionStatus: "Transaction status",
  pureComputation: "Local computation · no project access",
  projectComputation: "Project computation · selected files",
};

const activityMarks: Record<TaskActivityKind, string> = {
  modelReasoning: "✦",
  modelPass: "◌",
  fileRead: "↳",
  fileWrite: "↳",
  search: "⌕",
  proposedWrite: "+",
  permissionDecision: "✓",
  transactionPreparation: "◇",
  transactionStatus: "◆",
  pureComputation: "ƒ",
  projectComputation: "ƒ",
};

const statusLabels: Record<TaskActivityStatus, string> = {
  pending: "Pending",
  inProgress: "In progress",
  completed: "Completed",
  reused: "Reused",
  compacted: "Compacted",
  partial: "Partial",
  failed: "Failed",
  cancelled: "Stopped",
  approved: "Approved",
  denied: "Denied",
};

export function TaskActivityTimeline({
  reasoning,
  activities,
  items,
  active,
}: TaskActivityTimelineProps) {
  const [expanded, setExpanded] = useState(active);
  const contentId = useId();
  const content = useRef<HTMLDivElement | null>(null);
  const followLatest = useRef(true);
  const timelineItems: TaskActivityTimelineItem[] = items ?? [
    ...(reasoning ? [{ type: "reasoning" as const, id: "reasoning", text: reasoning }] : []),
    ...activities.map((activity) => ({
      type: "activity" as const,
      id: activity.id,
      activity,
    })),
  ];
  const itemCount = timelineItems.length;

  useEffect(() => {
    setExpanded(active);
  }, [active]);

  useEffect(() => {
    if (active && expanded && followLatest.current && content.current) {
      content.current.scrollTop = content.current.scrollHeight;
    }
  }, [active, expanded, timelineItems]);

  function handleScroll() {
    const element = content.current;
    if (!element) return;
    followLatest.current = element.scrollHeight - element.scrollTop - element.clientHeight
      <= BOTTOM_THRESHOLD_PX;
  }

  function toggleExpanded() {
    setExpanded((current) => {
      if (!current) followLatest.current = true;
      return !current;
    });
  }

  if (itemCount === 0) return null;

  return (
    <section className="task-timeline" data-active={active}>
      <button
        className="task-timeline-toggle"
        type="button"
        aria-expanded={expanded}
        aria-controls={contentId}
        onClick={toggleExpanded}
      >
        <span className="task-timeline-indicator" aria-hidden="true" />
        <span>Activity</span>
        <small>{active ? "Working" : `${itemCount} ${itemCount === 1 ? "item" : "items"}`}</small>
        <span className="task-timeline-chevron" aria-hidden="true">⌄</span>
      </button>
      {expanded ? (
        <div className="task-timeline-content" id={contentId} ref={content} onScroll={handleScroll}>
          {timelineItems.map((item, index) => {
            if (item.type === "image") {
              return (
                <article className="task-activity-item" key={item.id}>
                  <span className="task-activity-mark" aria-hidden="true">{activityMarks.fileRead}</span>
                  <div>
                    <header><strong>Image</strong></header>
                    <ManagedImagePreview assetId={item.assetId} alt="Image used during this response" />
                  </div>
                </article>
              );
            }
            if (item.type === "reasoning" || item.type === "commentary") {
              const inProgress = active && index === timelineItems.length - 1;
              return (
                <article className="task-activity-item" data-status={inProgress ? "inProgress" : "completed"} key={item.id}>
                  <span className="task-activity-mark" aria-hidden="true">
                    {activityMarks.modelReasoning}
                  </span>
                  <div>
                    <header>
                      <strong>{item.type === "commentary" ? "Progress update" : activityLabels.modelReasoning}</strong>
                      <small>{inProgress ? "In progress" : "Completed"}</small>
                    </header>
                    <p>{item.text}</p>
                  </div>
                </article>
              );
            }
            const { activity } = item;
            return (
              <article className="task-activity-item" data-status={activity.status} key={item.id}>
                <span className="task-activity-mark" aria-hidden="true">
                  {activityMarks[activity.kind]}
                </span>
                <div>
                  <header>
                    <strong>{activity.title || activityLabels[activity.kind]}</strong>
                    <small>{statusLabels[activity.status]}</small>
                  </header>
                  {activity.detail ? <p>{activity.detail}</p> : null}
                </div>
              </article>
            );
          })}
        </div>
      ) : null}
    </section>
  );
}
