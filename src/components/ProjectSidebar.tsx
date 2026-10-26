import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import type {
  Conversation,
  ConversationId,
  ConversationSummary,
  IpcError,
  ProjectSummary,
  RenameConversationRequest,
} from "../generated/ipc";
import type { ActiveChatResponse } from "./chat/generationActivity";

type RuntimeStatus = "connecting" | "ready" | "preview";

type ProjectSidebarProps = {
  runtimeStatus: RuntimeStatus;
  projects: ProjectSummary[];
  activeProject: ProjectSummary | null;
  selectedConversationId: ConversationId | null;
  onProjectSelected: (project: ProjectSummary) => void;
  onCreateProject: () => void;
  onConversationSelected: (conversation: Conversation | null) => void;
  onWalletSelected: () => void;
  onAppSettingsSelected: () => void;
  onBack: () => void;
  backLabel: string;
  settingsSelected: boolean;
  walletSelected: boolean;
  appSettingsSelected: boolean;
  historyVersion: number;
  projectPending: boolean;
  activeChatResponse: ActiveChatResponse | null;
};

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as IpcError).message);
  }
  return "Chat history could not be updated.";
}

function NavigationBackIcon() {
  return <svg className="navigation-back-glyph" viewBox="0 0 24 24" aria-hidden="true"><path d="m14 6-6 6 6 6" /></svg>;
}

export function ProjectSidebar({
  runtimeStatus,
  projects,
  activeProject,
  selectedConversationId,
  onProjectSelected,
  onCreateProject,
  onConversationSelected,
  onWalletSelected,
  onAppSettingsSelected,
  onBack,
  backLabel,
  settingsSelected,
  walletSelected,
  appSettingsSelected,
  historyVersion,
  projectPending,
  activeChatResponse,
}: ProjectSidebarProps) {
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [query, setQuery] = useState("");
  const [editingId, setEditingId] = useState<ConversationId | null>(null);
  const [editingTitle, setEditingTitle] = useState("");
  const [confirmingDeleteId, setConfirmingDeleteId] = useState<ConversationId | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const renameInput = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (runtimeStatus !== "ready" || activeProject === null) {
      setConversations([]);
      return;
    }
    let mounted = true;
    setPending(true);
    setError(null);
    invoke<ConversationSummary[]>("list_conversations", { projectId: activeProject.id })
      .then((history) => { if (mounted) setConversations(history); })
      .catch((loadError) => { if (mounted) setError(errorMessage(loadError)); })
      .finally(() => { if (mounted) setPending(false); });
    return () => { mounted = false; };
  }, [activeProject, historyVersion, runtimeStatus]);

  useEffect(() => {
    if (editingId !== null) renameInput.current?.focus();
  }, [editingId]);

  const filteredConversations = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    if (normalizedQuery.length === 0) return conversations;
    return conversations.filter((conversation) =>
      conversation.title.toLocaleLowerCase().includes(normalizedQuery),
    );
  }, [conversations, query]);

  async function selectConversation(conversationId: ConversationId) {
    if (pending || editingId !== null || confirmingDeleteId !== null) return;
    setPending(true);
    setError(null);
    try {
      const conversation = await invoke<Conversation | null>("load_conversation", { conversationId });
      if (conversation === null || conversation.projectId !== activeProject?.id) {
        setConversations((current) => current.filter((item) => item.id !== conversationId));
        setError("This chat is no longer available in this project.");
        if (selectedConversationId === conversationId) onConversationSelected(null);
        return;
      }
      onConversationSelected(conversation);
    } catch (loadError) {
      setError(errorMessage(loadError));
    } finally {
      setPending(false);
    }
  }

  function beginRename(conversation: ConversationSummary) {
    setEditingId(conversation.id);
    setEditingTitle(conversation.title);
    setConfirmingDeleteId(null);
    setError(null);
  }

  function cancelRename() {
    setEditingId(null);
    setEditingTitle("");
    setError(null);
  }

  async function renameConversation(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (editingId === null || pending) return;
    const title = editingTitle.trim();
    if (title.length === 0) {
      setError("Chat titles cannot be empty.");
      return;
    }
    const request: RenameConversationRequest = { conversationId: editingId, title };
    setPending(true);
    setError(null);
    try {
      const renamed = await invoke<ConversationSummary>("rename_conversation", { request });
      setConversations((current) => [renamed, ...current.filter((item) => item.id !== renamed.id)]);
      if (selectedConversationId === renamed.id) {
        onConversationSelected(await invoke<Conversation | null>("load_conversation", {
          conversationId: renamed.id,
        }));
      }
      cancelRename();
    } catch (renameError) {
      setError(errorMessage(renameError));
    } finally {
      setPending(false);
    }
  }

  async function deleteConversation(conversationId: ConversationId) {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      const deleted = await invoke<boolean>("delete_conversation", { conversationId });
      if (!deleted) setError("This chat was already deleted.");
      setConversations((current) => current.filter((item) => item.id !== conversationId));
      if (selectedConversationId === conversationId) onConversationSelected(null);
      setConfirmingDeleteId(null);
    } catch (deleteError) {
      setError(errorMessage(deleteError));
    } finally {
      setPending(false);
    }
  }

  return (
    <aside className="sidebar" aria-label="Project navigation">
      <div className="brand">
        <span className="brand-mark" aria-hidden="true"><img className="brand-symbol" src="/brand/minutus-mark.svg" alt="" /></span>
        <span className="brand-name">Minutus</span>
      </div>

      <div className="project-switcher-block">
        <label className="project-switcher-label" htmlFor="project-switcher-select">
          <span>Select project</span>
          {activeChatResponse ? (
            <span className="project-response-status" title={`${projects.find((project) => project.id === activeChatResponse.projectId)?.name ?? "Project"} is responding`}>
              <span className="response-indicator" aria-hidden="true" />
              {projects.find((project) => project.id === activeChatResponse.projectId)?.name ?? "Responding"}
            </span>
          ) : null}
        </label>
        <div className="project-switcher">
          <select
            id="project-switcher-select"
            value={activeProject?.id ?? ""}
            onChange={(event) => {
              const project = projects.find((item) => item.id === event.target.value);
              if (project) onProjectSelected(project);
            }}
            disabled={runtimeStatus !== "ready" || projectPending || projects.length === 0}
            aria-label="Current project"
          >
            {projects.length === 0 ? <option value="">No projects</option> : null}
            {projects.map((project) => (
              <option value={project.id} key={project.id}>
                {project.name}{activeChatResponse?.projectId === project.id ? " · Responding" : ""}
              </option>
            ))}
          </select>
          <button type="button" onClick={onCreateProject} disabled={runtimeStatus !== "ready" || projectPending} aria-label="Create project">
            {projectPending ? "…" : "+"}
          </button>
        </div>
      </div>

      <button
        className="new-chat"
        type="button"
        onClick={() => onConversationSelected(null)}
        disabled={runtimeStatus !== "ready" || activeProject === null || pending}
      >
        <span aria-hidden="true">+</span>New chat in project
      </button>

      <div className="conversation-search">
        <span aria-hidden="true">⌕</span>
        <input
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search chats in project"
          aria-label="Search project chats"
          disabled={runtimeStatus !== "ready" || activeProject === null}
        />
      </div>

      <nav className="conversation-history" aria-label="Project chat history">
        {filteredConversations.map((conversation) => {
          if (editingId === conversation.id) {
            return (
              <form
                className="conversation-edit"
                key={conversation.id}
                onSubmit={renameConversation}
                onBlur={(event) => {
                  if (!pending && !event.currentTarget.contains(event.relatedTarget)) cancelRename();
                }}
              >
                <input
                  ref={renameInput}
                  value={editingTitle}
                  onChange={(event) => setEditingTitle(event.target.value)}
                  onKeyDown={(event) => { if (event.key === "Escape") cancelRename(); }}
                  aria-label={`Rename ${conversation.title}`}
                  maxLength={160}
                  disabled={pending}
                />
                <button type="submit" disabled={pending}>Save</button>
                <button type="button" onClick={cancelRename} disabled={pending}>Cancel</button>
              </form>
            );
          }
          const confirmingDelete = confirmingDeleteId === conversation.id;
          const responding = activeChatResponse?.conversationId === conversation.id;
          return (
            <div className="conversation-row" data-selected={selectedConversationId === conversation.id && !settingsSelected} data-responding={responding} key={conversation.id}>
              <button className="conversation-select" type="button" onClick={() => selectConversation(conversation.id)} disabled={pending} aria-label={responding ? `${conversation.title}, responding` : conversation.title}>
                <span>{conversation.title}</span>
                {responding ? <span className="response-indicator" aria-hidden="true" /> : null}
              </button>
              {confirmingDelete ? (
                <div className="conversation-confirm" aria-label={`Delete ${conversation.title}?`}>
                  <button type="button" onClick={() => deleteConversation(conversation.id)} disabled={pending}>Delete</button>
                  <button type="button" onClick={() => setConfirmingDeleteId(null)} disabled={pending}>Cancel</button>
                </div>
              ) : (
                <div className="conversation-actions">
                  <button type="button" onClick={() => beginRename(conversation)}>Rename</button>
                  <button type="button" onClick={() => { setConfirmingDeleteId(conversation.id); setEditingId(null); }}>Delete</button>
                </div>
              )}
            </div>
          );
        })}
        {runtimeStatus === "ready" && !pending && activeProject !== null && filteredConversations.length === 0 ? (
          <p className="conversation-empty-copy">{query.trim() ? "No matching chats" : "No chats yet"}</p>
        ) : null}
      </nav>

      {error ? <p className="conversation-error" role="alert">{error}</p> : null}

      <div className="application-navigation">
        <button className="project-settings-link wallet-link" type="button" data-selected={walletSelected} onClick={walletSelected ? onBack : onWalletSelected} aria-label={walletSelected ? backLabel : undefined}>
          {walletSelected ? <NavigationBackIcon /> : <svg className="wallet-glyph" viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7.5A2.5 2.5 0 0 1 6.5 5h11A2.5 2.5 0 0 1 20 7.5V9h-5.5A2.5 2.5 0 0 0 12 11.5v1a2.5 2.5 0 0 0 2.5 2.5H20v1.5a2.5 2.5 0 0 1-2.5 2.5h-11A2.5 2.5 0 0 1 4 16.5v-9Z"/><path d="M14.5 10.5H21v3h-6.5a1 1 0 0 1-1-1v-1a1 1 0 0 1 1-1Z"/></svg>}
          {walletSelected ? "Back" : "Wallet"}
        </button>
        <button className="project-settings-link app-settings-link" type="button" data-selected={appSettingsSelected} onClick={appSettingsSelected ? onBack : onAppSettingsSelected} aria-label={appSettingsSelected ? backLabel : undefined}>
          {appSettingsSelected ? <NavigationBackIcon /> : <svg className="settings-glyph" viewBox="0 0 24 24" aria-hidden="true"><path d="m9 3-1 3-3 1-2 5 2 5 3 1 1 3h6l1-3 3-1 2-5-2-5-3-1-1-3Z" /><circle cx="12" cy="12" r="3" /></svg>}
          {appSettingsSelected ? "Back" : "Minutus settings"}
        </button>
      </div>

    </aside>
  );
}
