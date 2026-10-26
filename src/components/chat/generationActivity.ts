import type { ConversationId, ProjectId } from "../../generated/ipc";

/** Presentation identity for the response currently owned by the local runtime. */
export type ActiveChatResponse = {
  projectId: ProjectId;
  conversationId: ConversationId | null;
};
