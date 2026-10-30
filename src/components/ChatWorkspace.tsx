import { modelAvailabilityLabel } from "./models/availability";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ClipboardEvent, DragEvent, KeyboardEvent } from "react";
import type {
  AssetId,
  ChatEffort,
  ChatGenerationPhase,
  ChatGenerationAccepted,
  ChatGenerationEvent,
  ChatRequest,
  CatalogueModel,
  Conversation,
  ConversationId,
  FilesystemUndoRequest,
  FilesystemUndoResult,
  FilesystemWriteResult,
  ImageAttachmentReceipt,
  IpcError,
  Message,
  ModelSelection,
  ProjectId,
  ProjectWorkspaceSummary,
  RequestId,
  TaskActivity,
  ToolRequest,
  ToolCallId,
} from "../generated/ipc";
import { FilesystemChangeReview } from "./FilesystemChangeReview";
import { ManagedImagePreview } from "./ManagedImagePreview";
import { CopyTextButton, MarkdownContent } from "./MarkdownContent";
import { TaskActivityTimeline } from "./TaskActivityTimeline";
import type { TaskActivityTimelineItem } from "./chat/activityTimeline";
import { projectTranscript } from "./chat/transcript";
import { GenerationFeedback, MilestoneTrail } from "./chat/GenerationFeedback";
import { currentWorkLabel } from "./chat/feedbackProjection";
import { advanceRoundFeedback, initialRoundFeedback } from "./chat/roundFeedback";
import type { RoundFeedback } from "./chat/roundFeedback";
import {
  recordFeedbackPaint, recordFeedbackReceipt, recordSendAccepted, recordSendStart,
} from "./chat/feedbackMetrics";
import {
  emptyStreamOutput, hasFinalMessage, isActive, matchesGeneration,
  nextStreamPhase, reduceStreamOutput, visibleStreamText,
} from "./chat/streamOutput";
import type { GenerationPhase, StreamOutput } from "./chat/streamOutput";
import type { ActiveChatResponse } from "./chat/generationActivity";
import { DictationButton, DictationStatus } from "./chat/ComposerDictation";
import { ComposerSettings, EXTRA_LARGE_CONTEXT_TOKENS, capacityForModel } from "./chat/ComposerSettings";
import type { CapacityChoice } from "./chat/ComposerSettings";
import { shouldSubmitComposerKey } from "./chat/dictation";
import { useDictation } from "./chat/useDictation";

type GenerationState = StreamOutput & RoundFeedback & {
  projectId: ProjectId;
  requestId: RequestId | null;
  conversationId: ConversationId | null;
  phase: GenerationPhase;
  status: ChatGenerationPhase;
  reasoning: string;
  activities: TaskActivity[];
  timeline: TaskActivityTimelineItem[];
  lastSequence: number;
  error: string | null;
};

type ChatWorkspaceProps = {
  projectId: ProjectId;
  projectName: string;
  conversation: Conversation | null;
  models: CatalogueModel[];
  defaultModel: ModelSelection | null;
  onConversationChanged: (conversation: Conversation) => void;
  onHistoryChanged: () => void;
  onGenerationActivityChanged: (response: ActiveChatResponse | null) => void;
  hidden: boolean;
};

type PendingAttachment = {
  receipt: ImageAttachmentReceipt;
  label: string;
};

type WriteReview = {
  callId: ToolCallId;
  messageId: string;
  result: FilesystemWriteResult;
  undoResult: FilesystemUndoResult | null;
  recorded: boolean;
};

function filesystemWriteResult(value: unknown): FilesystemWriteResult | null {
  if (typeof value !== "object" || value === null || !("changes" in value)) return null;
  const changes = (value as { changes?: unknown }).changes;
  if (!Array.isArray(changes) || !changes.every((change) =>
    typeof change === "object"
    && change !== null
    && typeof (change as { relativePath?: unknown }).relativePath === "string"
    && typeof (change as { created?: unknown }).created === "boolean"
    && typeof (change as { newSha256?: unknown }).newSha256 === "string"
    && typeof (change as { diff?: unknown }).diff === "string"
  )) return null;
  return value as FilesystemWriteResult;
}

function collectWriteReviews(messages: Message[]): Map<string, WriteReview[]> {
  const reviews = new Map<ToolCallId, WriteReview>();
  let latestCallId: ToolCallId | null = null;
  for (const message of messages) {
    for (const part of message.parts) {
      if (part.type === "toolCall" && (
        part.request.name === "filesystem.write_text"
        || part.request.name === "filesystem.replace_text"
        || part.request.name === "filesystem.write_delimited_table"
      )) {
        reviews.set(part.request.callId, {
          callId: part.request.callId,
          messageId: message.id,
          result: { changes: [], postconditionVerified: false },
          undoResult: null,
          recorded: false,
        });
      }
      if (part.type === "toolResult" && part.result.outcome.type === "completed") {
        const review = reviews.get(part.result.callId);
        const value = part.result.outcome.content.find((item) => item.type === "json");
        const result = value?.type === "json" ? filesystemWriteResult(value.value) : null;
        if (review && result) review.result = result;
      }
      if (part.type === "filesystemWriteRecord") {
        const review = reviews.get(part.writeCallId);
        if (review) {
          review.recorded = true;
          latestCallId = part.writeCallId;
        }
      }
      if (part.type === "filesystemUndo") {
        const review = reviews.get(part.writeCallId);
        if (review) review.undoResult = part.result;
      }
    }
  }
  const byMessage = new Map<string, WriteReview[]>();
  for (const review of reviews.values()) {
    if (review.result.changes.length === 0) continue;
    const current = byMessage.get(review.messageId) ?? [];
    current.push({ ...review, recorded: review.recorded && review.callId === latestCallId });
    byMessage.set(review.messageId, current);
  }
  return byMessage;
}

const MAX_ATTACHMENTS = 4;
const MAX_IMAGE_BYTES = 20 * 1024 * 1024;

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as IpcError).message);
  }
  return "The response could not be generated. You can try again.";
}

function messageText(message: Message): string {
  return message.parts
    .filter((part) => part.type === "text")
    .map((part) => part.text)
    .join("");
}

function messageAssetIds(message: Message): AssetId[] {
  return message.parts.flatMap((part) => {
    if (part.type === "asset") return [part.assetId];
    if (part.type !== "toolResult" || part.result.outcome.type !== "completed") return [];
    return part.result.outcome.content.flatMap((content) =>
      content.type === "asset" ? [content.assetId] : [],
    );
  });
}

function precedingUserText(messages: Message[], beforeIndex = messages.length): string {
  for (let index = beforeIndex - 1; index >= 0; index -= 1) {
    if (messages[index].role === "user") return messageText(messages[index]);
  }
  return "";
}

function withoutPromptEcho(reasoning: string, prompt: string): string {
  const exactPrompt = prompt.trim();
  return exactPrompt.length > 0 && reasoning.startsWith(exactPrompt)
    ? reasoning.slice(exactPrompt.length).trimStart()
    : reasoning;
}

function activityFromToolRequest(request: ToolRequest): TaskActivity {
  switch (request.name) {
    case "lookup":
      return { id: request.callId, kind: "search", title: "Searched local knowledge", detail: null, status: "inProgress" };
    case "fetchUrl":
      return { id: request.callId, kind: "search", title: "Searched an approved source", detail: null, status: "inProgress" };
    case "web.retrieve":
      return { id: request.callId, kind: "search", title: "Retrieved an approved web resource", detail: null, status: "inProgress" };
    case "web.search":
      return { id: request.callId, kind: "search", title: "Searched the public web", detail: null, status: "inProgress" };
    case "contract.inspect":
    case "contract.read":
      return { id: request.callId, kind: "search", title: "Observed unsigned contract data", detail: null, status: "inProgress" };
    case "contract.request_workflow":
      return { id: request.callId, kind: "transactionPreparation", title: "Requested a source-qualified contract workflow", detail: null, status: "inProgress" };
    case "wallet.request_transfer":
      return { id: request.callId, kind: "transactionPreparation", title: "Requested a native-reviewed wallet transfer", detail: null, status: "inProgress" };
    case "x402.request":
      return { id: request.callId, kind: "transactionStatus", title: "Handled an x402 request", detail: null, status: "inProgress" };
    case "filesystem.list":
      return { id: request.callId, kind: "fileRead", title: "Listed workspace files", detail: null, status: "inProgress" };
    case "filesystem.find_paths":
      return { id: request.callId, kind: "fileRead", title: "Discovered workspace paths", detail: null, status: "inProgress" };
    case "filesystem.collect_evidence":
      return { id: request.callId, kind: "fileRead", title: "Collected workspace evidence", detail: null, status: "inProgress" };
    case "filesystem.stat":
      return { id: request.callId, kind: "fileRead", title: "Inspected a workspace path", detail: null, status: "inProgress" };
    case "filesystem.read_text":
    case "readWorkspaceFile":
      return { id: request.callId, kind: "fileRead", title: "Read a workspace file", detail: null, status: "inProgress" };
    case "filesystem.inspect_image":
      return { id: request.callId, kind: "fileRead", title: "Inspected workspace image", detail: null, status: "inProgress" };
    case "filesystem.search":
      return { id: request.callId, kind: "fileRead", title: "Searched workspace files", detail: null, status: "inProgress" };
    case "filesystem.scan":
      return { id: request.callId, kind: "fileRead", title: "Analyzed workspace files", detail: null, status: "inProgress" };
    case "filesystem.create_directory":
      return { id: request.callId, kind: "fileWrite", title: "Created workspace directory", detail: null, status: "inProgress" };
    case "filesystem.write_text":
      return { id: request.callId, kind: "fileWrite", title: "Updated workspace files", detail: null, status: "inProgress" };
    case "filesystem.replace_text":
      return { id: request.callId, kind: "fileWrite", title: "Updated workspace file", detail: null, status: "inProgress" };
    case "filesystem.write_delimited_table":
      return { id: request.callId, kind: "fileWrite", title: "Wrote delimited table", detail: null, status: "inProgress" };
    case "orchestration.transform_artifact":
      return { id: request.callId, kind: "fileWrite", title: "Transforming artifact", detail: null, status: "inProgress" };
    case "computation.run":
      return { id: request.callId, kind: "pureComputation", title: "Ran isolated computation without project access", detail: null, status: "inProgress" };
    case "computation.run_project":
      return { id: request.callId, kind: "projectComputation", title: "Computed over selected project files", detail: null, status: "inProgress" };
    case "orchestration.submit_plan":
      return { id: request.callId, kind: "search", title: "Execution plan", detail: null, status: "inProgress" };
    case "orchestration.submit_forecast":
      return { id: request.callId, kind: "search", title: "Forecast workspace work", detail: null, status: "inProgress" };
    case "orchestration.complete_stage":
      return { id: request.callId, kind: "search", title: "Completed a work stage", detail: null, status: "inProgress" };
    case "orchestration.recall":
      return { id: request.callId, kind: "search", title: "Recalled conversation context", detail: null, status: "inProgress" };
    case "orchestration.search_tools":
      return { id: request.callId, kind: "search", title: "Searched available tools", detail: null, status: "inProgress" };
    case "orchestration.refine_stage":
      return { id: request.callId, kind: "search", title: "Refined a work stage", detail: null, status: "inProgress" };
    case "orchestration.tool_correction":
      return { id: request.callId, kind: "search", title: "Corrected workspace operation", detail: null, status: "inProgress" };
    case "proposeWorkspaceChange":
      return { id: request.callId, kind: "proposedWrite", title: "Proposed a workspace change", detail: null, status: "inProgress" };
    case "proposeTransaction":
      return { id: request.callId, kind: "transactionPreparation", title: "Prepared a transaction", detail: null, status: "inProgress" };
  }
}

function upsertActivity(activities: TaskActivity[], activity: TaskActivity): TaskActivity[] {
  const index = activities.findIndex((current) => current.id === activity.id);
  if (index < 0) return [...activities, activity];
  return activities.map((current, currentIndex) => currentIndex === index ? activity : current);
}

function settleActivities(
  activities: TaskActivity[],
  status: "completed" | "failed" | "cancelled",
): TaskActivity[] {
  return activities.map((activity) =>
    activity.status === "pending" || activity.status === "inProgress"
      ? { ...activity, status }
      : activity,
  );
}

function appendTimelineReasoning(
  items: TaskActivityTimelineItem[],
  text: string,
  sequence: number,
): TaskActivityTimelineItem[] {
  const last = items.at(-1);
  if (last?.type === "reasoning") {
    return items.map((item, index) => index === items.length - 1
      ? { ...last, text: last.text + text }
      : item);
  }
  return [...items, { type: "reasoning", id: `reasoning-${sequence}`, text }];
}

function upsertTimelineActivity(
  items: TaskActivityTimelineItem[],
  activity: TaskActivity,
): TaskActivityTimelineItem[] {
  const index = items.findIndex((item) => item.type === "activity" && item.id === activity.id);
  if (index < 0) return [...items, { type: "activity", id: activity.id, activity }];
  return items.map((item, currentIndex) => currentIndex === index
    ? { type: "activity", id: activity.id, activity }
    : item);
}

function updateTimelineActivity(
  items: TaskActivityTimelineItem[],
  callId: ToolCallId,
  update: (activity: TaskActivity) => TaskActivity,
): TaskActivityTimelineItem[] {
  return items.map((item) => item.type === "activity" && item.id === callId
    ? { ...item, activity: update(item.activity) }
    : item);
}

function settleTimelineActivities(
  items: TaskActivityTimelineItem[],
  status: "completed" | "failed" | "cancelled",
): TaskActivityTimelineItem[] {
  return items.map((item) => item.type === "activity"
    && (item.activity.status === "pending" || item.activity.status === "inProgress")
    ? { ...item, activity: { ...item.activity, status } }
    : item);
}

function timelineWithoutPromptEcho(
  items: TaskActivityTimelineItem[],
  prompt: string,
): TaskActivityTimelineItem[] {
  const firstReasoning = items.findIndex((item) => item.type === "reasoning");
  if (firstReasoning < 0) return items;
  return items.map((item, index) => index === firstReasoning && item.type === "reasoning"
    ? { ...item, text: withoutPromptEcho(item.text, prompt) }
    : item).filter((item) => item.type !== "reasoning" || item.text.length > 0);
}

export function ChatWorkspace({
  projectId,
  projectName,
  conversation,
  models,
  defaultModel,
  onConversationChanged,
  onHistoryChanged,
  onGenerationActivityChanged,
  hidden,
}: ChatWorkspaceProps) {
  const [projectHasNoFolders, setProjectHasNoFolders] = useState(false);
  useEffect(() => {
    let current = true;
    setProjectHasNoFolders(false);
    if (!hidden) {
      void invoke<ProjectWorkspaceSummary[]>("list_project_workspaces", { projectId })
        .then((folders) => { if (current) setProjectHasNoFolders(folders.length === 0); })
        .catch(() => { /* An unknown folder state must not show the empty-project hint. */ });
    }
    return () => { current = false; };
  }, [projectId, hidden]);
  const [draft, setDraft] = useState("");
  const [attachments, setAttachments] = useState<PendingAttachment[]>([]);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [importingAttachments, setImportingAttachments] = useState(false);
  const [draggingImages, setDraggingImages] = useState(false);
  const [generation, setGeneration] = useState<GenerationState | null>(null);
  const [selectedModel, setSelectedModel] = useState<ModelSelection | null>(null);
  const selectedModelAvailable = models.some((candidate) => candidate.installed && candidate.chatSelectable && candidate.id === selectedModel?.modelId && candidate.version === selectedModel.modelVersion);
  const [modelLoading, setModelLoading] = useState(true);
  const [modelError, setModelError] = useState<string | null>(null);
  const [capacity, setCapacity] = useState<CapacityChoice>("default");
  const [effort, setEffort] = useState<ChatEffort | null>("quick");
  const textarea = useRef<HTMLTextAreaElement | null>(null);
  const draftRevision = useRef(0);
  const activeConversation = useRef(conversation);
  const activeProjectId = useRef(projectId);
  const historyRefreshEpoch = useRef(0);
  const chatScroll = useRef<HTMLDivElement | null>(null);
  const transcriptEnd = useRef<HTMLDivElement | null>(null);
  const followTranscript = useRef(true);
  const dragDepth = useRef(0);
  const settingsKey = `minutus-response-settings:${conversation?.id ?? `${projectId}:new`}`;
  const writeReviews = useMemo(
    () => collectWriteReviews(conversation?.messages ?? []),
    [conversation?.messages],
  );
  const transcript = useMemo(() => projectTranscript(conversation?.messages ?? []), [conversation?.messages]);

  const updateDraft = useCallback((value: string) => {
    setDraft((current) => {
      if (current !== value) draftRevision.current += 1;
      return value;
    });
  }, []);

  useEffect(() => {
    activeConversation.current = conversation;
  }, [conversation]);

  useEffect(() => {
    activeProjectId.current = projectId;
  }, [projectId]);

  useEffect(() => {
    followTranscript.current = true;
  }, [conversation?.id]);

  useEffect(() => {
    let parsed: { capacity?: CapacityChoice | number; effort?: string | null } | null = null;
    try {
      const saved = window.localStorage.getItem(settingsKey);
      parsed = saved ? JSON.parse(saved) as { capacity?: CapacityChoice | number; effort?: string | null } : null;
    } catch {
      parsed = null;
    }
    const savedCapacity = parsed?.capacity;
    setCapacity(savedCapacity === "small" || savedCapacity === 16_384 ? "small"
      : savedCapacity === "large" || savedCapacity === 49_152 ? "large"
        : savedCapacity === "extraLarge" || savedCapacity === EXTRA_LARGE_CONTEXT_TOKENS ? "extraLarge"
          : savedCapacity === "max" || savedCapacity === 65_536 ? "max" : "default");
    setEffort(parsed?.effort === null ? null
      : parsed?.effort === "balanced" || parsed?.effort === "deep" ? parsed.effort : "quick");
  }, [settingsKey]);

  useEffect(() => {
    let mounted = true;
    setModelLoading(true);
    setModelError(null);
    setSelectedModel(null);
    const fallback = models.find((candidate) => candidate.installed && candidate.chatSelectable
      && candidate.id === defaultModel?.modelId && candidate.version === defaultModel.modelVersion)
      ?? models.find((candidate) => candidate.installed && candidate.chatSelectable && candidate.recommended)
      ?? models.find((candidate) => candidate.installed && candidate.chatSelectable);
    const fallbackSelection = fallback ? { modelId: fallback.id, modelVersion: fallback.version } : null;
    const command = conversation ? "conversation_model" : "project_model";
    const args = conversation ? { conversationId: conversation.id } : { projectId };
    void invoke<ModelSelection | null>(command, args).then((selection) => {
      if (!mounted) return;
      const configured = selection ?? (conversation ? null : defaultModel);
      const available = models.some((candidate) => candidate.installed && candidate.chatSelectable
        && candidate.id === configured?.modelId && candidate.version === configured.modelVersion);
      if (configured && !available) {
        setSelectedModel(configured);
        const registered = models.find((candidate) => candidate.id === configured.modelId && candidate.version === configured.modelVersion);
        setModelError(registered ? `${modelAvailabilityLabel(registered)}. Open Settings to locate the files or choose another available model.` : "The chosen model is not installed. Choose an available model to continue.");
      } else {
        setSelectedModel(available ? configured : fallbackSelection);
      }
    }).catch((error) => {
      if (!mounted) return;
      setSelectedModel(fallbackSelection);
      setModelError(errorMessage(error));
    }).finally(() => { if (mounted) setModelLoading(false); });
    return () => { mounted = false; };
  }, [conversation?.id, projectId, models, defaultModel, hidden]);

  function updateResponseSettings(nextCapacity: CapacityChoice, nextEffort: ChatEffort | null) {
    setCapacity(nextCapacity);
    setEffort(nextEffort);
    window.localStorage.setItem(settingsKey, JSON.stringify({ capacity: nextCapacity, effort: nextEffort }));
  }

  useEffect(() => {
    if (capacity !== "max" || selectedModel === null) return;
    const selected = models.find((candidate) => candidate.id === selectedModel.modelId
      && candidate.version === selectedModel.modelVersion);
    if (selected && selected.contextLimitTokens <= EXTRA_LARGE_CONTEXT_TOKENS) {
      updateResponseSettings("extraLarge", effort);
    }
  }, [capacity, effort, models, selectedModel]);

  useEffect(() => {
    if (followTranscript.current) transcriptEnd.current?.scrollIntoView({ block: "end" });
  }, [
    conversation?.messages.length,
    generation?.timeline,
    generation?.text,
    generation?.previewText,
  ]);

  useEffect(() => {
    if (generation === null || generation.requestId === null || generation.lastSequence < 0) return;
    const sequence = generation.lastSequence;
    const requestId = generation.requestId;
    const frame = window.requestAnimationFrame(() => recordFeedbackPaint(requestId, sequence));
    return () => window.cancelAnimationFrame(frame);
  }, [generation?.lastSequence]);

  function handleTranscriptScroll() {
    const element = chatScroll.current;
    if (!element) return;
    followTranscript.current = element.scrollHeight - element.scrollTop - element.clientHeight
      <= 32;
  }

  useEffect(() => {
    let mounted = true;
    let stopListening: (() => void) | undefined;

    listen<ChatGenerationEvent>("chat-generation-event", ({ payload }) => {
      if (!mounted) return;
      const event = payload.event;
      const receivedAt = Date.now();
      recordFeedbackReceipt(event);
      setGeneration((current) => {
        if (current === null || !isActive(current.phase)) return current;
        if (current.requestId !== null && current.requestId !== event.requestId) return current;
        if (current.conversationId !== null && current.conversationId !== payload.conversationId) {
          return current;
        }
        if (event.sequence <= current.lastSequence) return current;

        const identified = {
          ...current,
          ...reduceStreamOutput(current, event),
          ...advanceRoundFeedback(current, event, receivedAt),
          phase: nextStreamPhase(current.phase, event),
          requestId: event.requestId,
          conversationId: payload.conversationId,
          lastSequence: event.sequence,
        };
        switch (event.type) {
          case "status":
            return { ...identified, status: event.phase };
          case "textPreviewDelta":
          case "textDelta":
          case "textPreviewReset":
            return identified;
          case "reasoningDelta":
            return {
              ...identified,
              reasoning: current.reasoning + event.text,
              timeline: appendTimelineReasoning(current.timeline, event.text, event.sequence),
            };
          case "activity":
            return {
              ...identified,
              activities: upsertActivity(current.activities, event.activity),
              timeline: upsertTimelineActivity(current.timeline, event.activity),
            };
          case "cancelled":
            return {
              ...identified,
              activities: settleActivities(current.activities, "cancelled"),
              timeline: settleTimelineActivities(current.timeline, "cancelled"),
            };
          case "error":
            return {
              ...identified,
              activities: settleActivities(current.activities, "failed"),
              timeline: settleTimelineActivities(current.timeline, "failed"),
              error: event.error.message,
            };
          case "final":
            return {
              ...identified,
              activities: settleActivities(current.activities, "completed"),
              timeline: settleTimelineActivities(current.timeline, "completed"),
            };
          case "toolCall": {
            const activity = activityFromToolRequest(event.request);
            return {
              ...identified,
              activities: upsertActivity(current.activities, activity),
              timeline: upsertTimelineActivity(current.timeline, activity),
            };
          }
          case "toolResultNeeded":
            return {
              ...identified,
              activities: current.activities.map((activity) =>
                activity.id === event.callId ? { ...activity, status: "pending" } : activity,
              ),
              timeline: updateTimelineActivity(current.timeline, event.callId, (activity) => ({
                ...activity,
                status: "pending",
              })),
            };
        }
      });
    })
      .then((unlisten) => {
        if (mounted) stopListening = unlisten;
        else unlisten();
      })
      .catch(() => {
        if (mounted) {
          setGeneration({
            projectId: activeProjectId.current,
            requestId: null,
            conversationId: activeConversation.current?.id ?? null,
            phase: "error",
            status: "startingResponse",
            ...initialRoundFeedback(Date.now()),
            ...emptyStreamOutput(),
            reasoning: "",
            activities: [],
            timeline: [],
            lastSequence: -1,
            error: "Live response updates are unavailable. Restart Minutus and try again.",
          });
        }
      });

    return () => {
      mounted = false;
      stopListening?.();
    };
  }, []);

  // History IO is a terminal-state effect, never a side effect of React's state
  // updater. A later request invalidates an outstanding refresh; navigation
  // is checked separately before updating the visible conversation.
  useEffect(() => {
    const requestId = generation?.requestId;
    const conversationId = generation?.conversationId;
    const phase = generation?.phase;
    const messageId = generation?.finalMessageId;
    if (!requestId || !conversationId || (phase !== "settled" && phase !== "error")) return;
    let mounted = true;
    const epoch = historyRefreshEpoch.current;
    void invoke<Conversation | null>("load_conversation", { conversationId }).then((refreshed) => {
      if (!mounted || epoch !== historyRefreshEpoch.current) return;
      if (!refreshed || (messageId && !hasFinalMessage(refreshed.messages, messageId))) {
        throw new Error("Final message unavailable in refreshed history");
      }
      if (activeConversation.current?.id === conversationId) onConversationChanged(refreshed);
      onHistoryChanged();
      if (phase === "settled") {
        setGeneration((current) => current?.phase === "settled"
          && matchesGeneration(current, requestId, conversationId) ? null : current);
      }
    }).catch(() => {
      if (!mounted || epoch !== historyRefreshEpoch.current || phase === "error") return;
      setGeneration((current) => current && matchesGeneration(current, requestId, conversationId)
        ? { ...current, error: "The response finished, but history could not be refreshed. Reopen this conversation to recover it." }
        : current);
    });
    return () => { mounted = false; };
  }, [generation?.requestId, generation?.conversationId, generation?.phase,
    generation?.finalMessageId, onConversationChanged, onHistoryChanged]);

  const generating = generation !== null && isActive(generation.phase);
  useEffect(() => {
    onGenerationActivityChanged(generating && generation
      ? { projectId: generation.projectId, conversationId: generation.conversationId }
      : null);
  }, [generation?.projectId, generation?.conversationId, generating, onGenerationActivityChanged]);
  const dictation = useDictation({
    projectId,
    conversationId: conversation?.id ?? null,
    draft,
    draftRevision: draftRevision.current,
    textarea,
    disabled: generating || importingAttachments,
    onDraftChange: updateDraft,
  });
  const visibleGeneration = (generation?.conversationId === null
    || generation?.conversationId === conversation?.id)
    && !(generation?.phase === "settled" && hasFinalMessage(conversation?.messages ?? [], generation.finalMessageId))
    ? generation
    : null;
  const visibleReasoning = visibleGeneration
    ? withoutPromptEcho(
        visibleGeneration.reasoning,
        precedingUserText(conversation?.messages ?? []),
      )
    : "";
  const visibleTimeline = visibleGeneration
    ? timelineWithoutPromptEcho(
        visibleGeneration.timeline,
        precedingUserText(conversation?.messages ?? []),
      )
    : [];
  const visibleWorkLabel = visibleGeneration
    ? currentWorkLabel(
        visibleGeneration.status,
        visibleGeneration.activities,
        visibleGeneration.previewText.length > 0,
        visibleGeneration.roundHasAnswer,
        visibleGeneration.roundHasReasoning,
      )
    : "";
  const hasTranscript = (conversation?.messages.length ?? 0) > 0 || visibleGeneration !== null;

  async function undoFilesystemWrite(writeCallId: ToolCallId): Promise<Conversation> {
    if (!conversation) throw new Error("The conversation is unavailable.");
    const request: FilesystemUndoRequest = {
      projectId,
      conversationId: conversation.id,
      writeCallId,
    };
    const updated = await invoke<Conversation>("undo_filesystem_write", { request });
    onHistoryChanged();
    return updated;
  }

  async function checkFilesystemUndo(writeCallId: ToolCallId): Promise<boolean> {
    if (!conversation) return false;
    const request: FilesystemUndoRequest = {
      projectId,
      conversationId: conversation.id,
      writeCallId,
    };
    return invoke<boolean>("filesystem_undo_available", { request });
  }

  async function sendMessage() {
    const text = draft.trim();
    if (text.length === 0 || generating || importingAttachments || dictation.active || modelLoading || selectedModel === null) return;
    const selectedCatalogue = models.find((candidate) => candidate.id === selectedModel.modelId
      && candidate.version === selectedModel.modelVersion);
    if (!selectedCatalogue?.installed) {
      setModelError("The chosen model is no longer available. Choose an installed model to continue.");
      return;
    }
    const sendStarted = recordSendStart();
    historyRefreshEpoch.current += 1;
    const request: ChatRequest = {
      projectId,
      conversationId: conversation?.id ?? null,
      text,
      assetIds: attachments.map(({ receipt }) => receipt.assetId),
      modelSelection: selectedModel,
      contextTokens: capacity === "small" ? 16_384 : capacity === "large" ? 65_536
        : capacityForModel(capacity, selectedCatalogue.contextLimitTokens) === "extraLarge"
          ? EXTRA_LARGE_CONTEXT_TOKENS : capacity === "max" ? selectedCatalogue.contextLimitTokens : null,
      effort,
    };
    updateDraft("");
    followTranscript.current = true;
    setGeneration({
      projectId,
      requestId: null,
      conversationId: conversation?.id ?? null,
      phase: "preparing",
      status: "startingResponse",
      ...initialRoundFeedback(Date.now()),
      ...emptyStreamOutput(),
      reasoning: "",
      activities: [],
      timeline: [],
      lastSequence: -1,
      error: null,
    });
    try {
      const accepted = await invoke<ChatGenerationAccepted>("start_chat_generation", { request });
      if (conversation === null) {
        window.localStorage.setItem(
          `minutus-response-settings:${accepted.conversation.id}`,
          JSON.stringify({ capacity, effort }),
        );
      }
      recordSendAccepted(sendStarted);
      setAttachments([]);
      setAttachmentError(null);
      activeConversation.current = accepted.conversation;
      onConversationChanged(accepted.conversation);
      onHistoryChanged();
      setGeneration((current) => current && isActive(current.phase)
        ? {
            ...current,
            requestId: accepted.requestId,
            conversationId: accepted.conversation.id,
          }
        : current);
    } catch (error) {
      setGeneration((current) => ({
        ...emptyStreamOutput(),
        projectId: current?.projectId ?? projectId,
        requestId: current?.requestId ?? null,
        conversationId: current?.conversationId ?? conversation?.id ?? null,
        phase: "error",
        status: current?.status ?? "startingResponse",
        text: current?.text ?? "",
        reasoning: current?.reasoning ?? "",
        activities: current?.activities ?? [],
        timeline: current?.timeline ?? [],
        lastSequence: current?.lastSequence ?? -1,
        error: errorMessage(error),
        currentRound: current?.currentRound ?? -1,
        roundStartedAt: current?.roundStartedAt ?? Date.now(),
        roundHasReasoning: current?.roundHasReasoning ?? false,
        roundHasAnswer: current?.roundHasAnswer ?? false,
      }));
    }
  }

  async function addFiles(files: File[]) {
    if (generating || importingAttachments || dictation.active || files.length === 0) return;
    const remaining = MAX_ATTACHMENTS - attachments.length;
    if (remaining <= 0 || files.length > remaining) {
      setAttachmentError(`You can attach up to ${MAX_ATTACHMENTS} images.`);
      return;
    }
    const images = files.filter((file) => file.type.startsWith("image/"));
    if (images.length !== files.length) {
      setAttachmentError("Only static PNG, JPEG and WebP images can be attached.");
      if (images.length === 0) return;
    } else {
      setAttachmentError(null);
    }
    if (images.some((file) => file.size > MAX_IMAGE_BYTES)) {
      setAttachmentError("Images must be 20 MiB or smaller.");
      return;
    }

    setImportingAttachments(true);
    const imported: PendingAttachment[] = [];
    try {
      for (const file of images) {
        const bytes = await file.arrayBuffer();
        const receipt = await invoke<ImageAttachmentReceipt>("import_chat_image", bytes);
        imported.push({ receipt, label: file.name || "Pasted image" });
      }
      setAttachments((current) => [...current, ...imported]);
    } catch (error) {
      await Promise.allSettled(
        imported.map(({ receipt }) =>
          invoke("discard_chat_image", { assetId: receipt.assetId }),
        ),
      );
      setAttachmentError(errorMessage(error));
    } finally {
      setImportingAttachments(false);
    }
  }

  async function pickImages() {
    if (generating || importingAttachments || dictation.active) return;
    const remaining = MAX_ATTACHMENTS - attachments.length;
    if (remaining <= 0) {
      setAttachmentError(`You can attach up to ${MAX_ATTACHMENTS} images.`);
      return;
    }
    setImportingAttachments(true);
    setAttachmentError(null);
    try {
      const receipts = await invoke<ImageAttachmentReceipt[] | null>("pick_chat_images", {
        limit: remaining,
      });
      if (receipts !== null) {
        setAttachments((current) => [
          ...current,
          ...receipts.map((receipt, index) => ({
            receipt,
            label: receipts.length === 1 ? "Selected image" : `Selected image ${index + 1}`,
          })),
        ]);
      }
    } catch (error) {
      setAttachmentError(errorMessage(error));
    } finally {
      setImportingAttachments(false);
    }
  }

  function removeAttachment(assetId: AssetId) {
    setAttachments((current) => current.filter(({ receipt }) => receipt.assetId !== assetId));
    void invoke("discard_chat_image", { assetId }).catch((error) => {
      setAttachmentError(errorMessage(error));
    });
  }

  function handlePaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    const itemFiles = Array.from(event.clipboardData.items)
      .filter((item) => item.kind === "file")
      .map((item) => item.getAsFile())
      .filter((file): file is File => file !== null);
    const files = itemFiles.length > 0 ? itemFiles : Array.from(event.clipboardData.files);
    if (files.some((file) => file.type.startsWith("image/"))) {
      event.preventDefault();
      void addFiles(files);
    }
  }

  function handleDragEnter(event: DragEvent<HTMLElement>) {
    event.preventDefault();
    if (!event.dataTransfer.types.includes("Files")) return;
    dragDepth.current += 1;
    if (!generating && !dictation.active) setDraggingImages(true);
  }

  function handleDragLeave(event: DragEvent<HTMLElement>) {
    event.preventDefault();
    dragDepth.current = Math.max(0, dragDepth.current - 1);
    if (dragDepth.current === 0) setDraggingImages(false);
  }

  function handleDragOver(event: DragEvent<HTMLElement>) {
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  }

  function handleDrop(event: DragEvent<HTMLElement>) {
    event.preventDefault();
    dragDepth.current = 0;
    setDraggingImages(false);
    void addFiles(Array.from(event.dataTransfer.files));
  }

  async function stopGeneration() {
    if (generation?.requestId === null || !generating) return;
    const requestId = generation.requestId;
    setGeneration((current) => current ? { ...current, phase: "stopping" } : current);
    try {
      const accepted = await invoke<boolean>("stop_chat_generation", { requestId });
      if (!accepted) {
        setGeneration((current) => current ? {
          ...current,
          phase: "error",
          error: "This response had already finished. You can send another message.",
        } : current);
      }
    } catch (error) {
      setGeneration((current) => current ? {
        ...current,
        phase: "error",
        error: errorMessage(error),
      } : current);
    }
  }

  function handleComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey && dictation.active) {
      event.preventDefault();
      return;
    }
    if (shouldSubmitComposerKey(event.key, event.shiftKey, dictation.active)) {
      event.preventDefault();
      void sendMessage();
    }
  }

  return (
    <section
      className="chat-workspace"
      aria-label="Conversation"
      hidden={hidden}
      data-dragging-images={draggingImages}
      onDragEnter={handleDragEnter}
      onDragLeave={handleDragLeave}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      {draggingImages ? (
        <div className="attachment-drop-overlay" aria-hidden="true">
          <span>Drop images to attach</span>
        </div>
      ) : null}
      <div className="chat-scroll" ref={chatScroll} onScroll={handleTranscriptScroll}>
        {!hasTranscript ? (
          <div className="conversation-stage">
            <span className="hero-mark" aria-hidden="true">
              <img className="brand-symbol" src="/brand/minutus-mark.svg" alt="" />
            </span>
            <p className="eyebrow">Private by default</p>
            <h1>What would you like to build in {projectName}?</h1>
            <p className="welcome-copy">Ask Minutus anything. Your conversation stays on this device.</p>
            {projectHasNoFolders ? <p className="project-folder-hint">You can add folders to this project in Project settings.</p> : null}
          </div>
        ) : (
          <div className="transcript" aria-live="polite">
            {transcript.map((entry) => {
              const message = entry.type === "user" ? entry.message : entry.answer;
              const text = message ? messageText(message) : "";
              const assetIds = message ? messageAssetIds(message) : [];
              const activities = entry.type === "response" ? entry.activities : [];
              const reviews = entry.type === "response"
                ? entry.messageIds.flatMap((id) => writeReviews.get(id) ?? []) : [];
              const timeline = entry.type === "response" ? entry.timeline : [];
              if (text.length === 0 && reviews.length === 0 && assetIds.length === 0 && timeline.length === 0) return null;
              return (
                <article className={`chat-message chat-message-${entry.type === "user" ? "user" : "assistant"}`} key={entry.id}>
                  <span>{entry.type === "user" ? "You" : "Minutus"}</span>
                  <TaskActivityTimeline
                    reasoning=""
                    activities={activities}
                    items={timeline}
                    active={false}
                  />
                  {reviews.map((review) => (
                    <FilesystemChangeReview
                      key={review.callId}
                      callId={review.callId}
                      result={review.result}
                      undoResult={review.undoResult}
                      canUndo={review.recorded && review.undoResult === null}
                      disabled={generating}
                      onUndo={undoFilesystemWrite}
                      onCheckUndo={checkFilesystemUndo}
                      onConversationChanged={onConversationChanged}
                    />
                  ))}
                  {assetIds.length > 0 ? (
                    <div className="message-attachments" aria-label="Attached images">
                      {assetIds.map((assetId, index) => (
                        <ManagedImagePreview
                          key={assetId}
                          assetId={assetId}
                          alt={`Attached image ${index + 1}`}
                        />
                      ))}
                    </div>
                  ) : null}
                  {entry.type === "response" && text.length > 0 ? (
                    <>
                      <MarkdownContent>{text}</MarkdownContent>
                      <div className="message-actions">
                        <CopyTextButton text={text} label="Copy response" className="message-copy-action" />
                      </div>
                    </>
                  ) : text.length > 0 ? <p>{text}</p> : null}
                </article>
              );
            })}
            {visibleGeneration ? (
              <article className="chat-message chat-message-assistant chat-message-streaming">
                <span>Minutus</span>
                {isActive(visibleGeneration.phase) ? (
                  <GenerationFeedback
                    key={visibleGeneration.requestId ?? "pending"}
                    label={visibleWorkLabel}
                    startedAt={visibleGeneration.roundStartedAt}
                    round={visibleGeneration.currentRound}
                    activities={visibleGeneration.activities}
                    stopping={visibleGeneration.phase === "stopping"}
                    canStop={visibleGeneration.requestId !== null}
                    onStop={stopGeneration}
                  />
                ) : <MilestoneTrail activities={visibleGeneration.activities} />}
                <TaskActivityTimeline
                  reasoning={visibleReasoning}
                  activities={visibleGeneration.activities}
                  items={visibleTimeline}
                  active={isActive(visibleGeneration.phase) && visibleStreamText(visibleGeneration).length === 0}
                />
                {visibleStreamText(visibleGeneration) ? (
                  <div data-provisional={visibleGeneration.previewText.length > 0} className="generation-answer">
                    <MarkdownContent>{visibleStreamText(visibleGeneration)}</MarkdownContent>
                  </div>
                ) : null}
                {visibleGeneration.phase === "cancelled" ? (
                  <p className="generation-note">Stopped. The partial response was not added to history.</p>
                ) : null}
                {visibleGeneration.error ? (
                  <div className="generation-error" role="alert">
                    <p>{visibleGeneration.error}</p>
                    <button type="button" onClick={() => setGeneration(null)}>Dismiss</button>
                  </div>
                ) : null}
              </article>
            ) : null}
            <div ref={transcriptEnd} />
          </div>
        )}
      </div>

      <div className="composer-wrap">
        <div className="composer" data-generating={generating} data-dictating={dictation.active}>
          {attachments.length > 0 ? (
            <div className="composer-attachments" aria-label="Images ready to send">
              {attachments.map(({ receipt, label }) => (
                <figure className="composer-attachment" key={receipt.assetId}>
                  <ManagedImagePreview assetId={receipt.assetId} alt={label} />
                  <button
                    type="button"
                    onClick={() => removeAttachment(receipt.assetId)}
                    aria-label={`Remove ${label}`}
                    disabled={generating || dictation.active}
                  >
                    ×
                  </button>
                </figure>
              ))}
            </div>
          ) : null}
          <button
            className="attach-action"
            type="button"
            onClick={pickImages}
            disabled={generating || dictation.active || importingAttachments || attachments.length >= MAX_ATTACHMENTS}
            aria-label="Attach images"
            title="Attach images"
          >
            <span aria-hidden="true">＋</span>
          </button>
          <textarea
            ref={textarea}
            aria-label="Message Minutus"
            aria-readonly={dictation.active}
            placeholder="Ask Minutus…"
            value={draft}
            onChange={(event) => updateDraft(event.target.value)}
            onKeyDown={handleComposerKeyDown}
            onPaste={handlePaste}
            disabled={generating}
            readOnly={dictation.active}
            rows={1}
          />
          <DictationButton
            state={dictation.state}
            disabled={generating || importingAttachments}
            onCaptureSelection={dictation.captureSelection}
            onStart={dictation.start}
            onFinish={dictation.finish}
          />
          {generating ? (
            <button
              className="stop-action"
              type="button"
              onClick={stopGeneration}
              disabled={generation?.requestId === null || generation.phase === "stopping"}
            >
              <span aria-hidden="true" />
              {generation?.phase === "stopping" ? "Stopping" : "Stop"}
            </button>
          ) : (
            <button
              className="send-action"
              type="button"
              onClick={sendMessage}
              disabled={draft.trim().length === 0 || importingAttachments || dictation.active || modelLoading || !selectedModelAvailable}
              aria-label="Send message"
            >
              ↑
            </button>
          )}
          <ComposerSettings
            models={models}
            model={selectedModel}
            capacity={capacity}
            effort={effort}
            disabled={generating || dictation.active}
            loadingModel={modelLoading}
            onModelChange={(choice) => { setSelectedModel(choice); setModelError(null); }}
            onCapacityChange={(next) => updateResponseSettings(next, effort)}
            onEffortChange={(next) => updateResponseSettings(capacity, next)}
          />
        </div>
        <DictationStatus
          state={dictation.state}
          onCancel={dictation.cancel}
          onDismiss={dictation.dismiss}
        />
        {attachmentError ? (
          <div className="attachment-error" role="alert">
            <span>{attachmentError}</span>
            <button type="button" onClick={() => setAttachmentError(null)}>Dismiss</button>
          </div>
        ) : null}
        {modelError ? <p className="composer-setting-error" role="alert">{modelError}</p> : null}
        <p className="composer-hint">
          {importingAttachments
            ? "Preparing images…"
            : dictation.active
              ? "The draft is locked during dictation · Escape to cancel"
              : "Enter to send · Shift + Enter for a new line"}
        </p>
      </div>
    </section>
  );
}
