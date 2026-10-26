import type { AssetId, TaskActivity } from "../../generated/ipc";

export type TaskActivityTimelineItem =
  | { type: "reasoning"; id: string; text: string }
  | { type: "commentary"; id: string; text: string }
  | { type: "activity"; id: string; activity: TaskActivity }
  | { type: "image"; id: string; assetId: AssetId };
