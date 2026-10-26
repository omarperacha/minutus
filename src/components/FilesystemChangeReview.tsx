import { useEffect, useState } from "react";
import type {
  Conversation,
  FilesystemUndoResult,
  FilesystemWriteResult,
  ToolCallId,
} from "../generated/ipc";

type FilesystemChangeReviewProps = {
  callId: ToolCallId;
  result: FilesystemWriteResult;
  undoResult: FilesystemUndoResult | null;
  canUndo: boolean;
  disabled: boolean;
  onUndo: (callId: ToolCallId) => Promise<Conversation>;
  onCheckUndo: (callId: ToolCallId) => Promise<boolean>;
  onConversationChanged: (conversation: Conversation) => void;
};

function undoStatus(result: FilesystemUndoResult): string {
  const undone = result.files.filter((file) => file.status === "undone").length;
  const conflicts = result.files.filter((file) => file.status === "conflict").length;
  const failed = result.files.length - undone - conflicts;
  if (undone === result.files.length) return "All changes were undone.";
  return [
    `${undone} undone`,
    conflicts > 0 ? `${conflicts} changed since application` : null,
    failed > 0 ? `${failed} could not be restored safely` : null,
  ].filter(Boolean).join(" · ");
}

function diffLineClass(line: string): string | undefined {
  if (line.startsWith("+++") || line.startsWith("---")) return "diff-file";
  if (line.startsWith("+")) return "diff-addition";
  if (line.startsWith("-")) return "diff-removal";
  if (line.startsWith("@@")) return "diff-hunk";
  return undefined;
}

function Diff({ value }: { value: string }) {
  return (
    <pre>
      {value.split("\n").map((line, index) => (
        <span className={diffLineClass(line)} key={`${index}-${line}`}>
          {line || " "}
        </span>
      ))}
    </pre>
  );
}

export function FilesystemChangeReview({
  callId,
  result,
  undoResult,
  canUndo,
  disabled,
  onUndo,
  onCheckUndo,
  onConversationChanged,
}: FilesystemChangeReviewProps) {
  const [expanded, setExpanded] = useState(false);
  const [undoing, setUndoing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [undoAvailable, setUndoAvailable] = useState<boolean | null>(null);

  useEffect(() => {
    if (!canUndo || undoResult) {
      setUndoAvailable(false);
      return;
    }
    let mounted = true;
    setUndoAvailable(null);
    onCheckUndo(callId)
      .then((available) => {
        if (mounted) setUndoAvailable(available);
      })
      .catch(() => {
        if (mounted) setUndoAvailable(false);
      });
    return () => {
      mounted = false;
    };
  }, [callId, canUndo, onCheckUndo, undoResult]);

  async function undo() {
    setUndoing(true);
    setError(null);
    try {
      onConversationChanged(await onUndo(callId));
    } catch (cause) {
      setError(
        typeof cause === "object" && cause !== null && "message" in cause
          ? String(cause.message)
          : "The changes could not be undone safely.",
      );
    } finally {
      setUndoing(false);
    }
  }

  return (
    <section className="change-review" aria-label="Applied filesystem changes">
      <header>
        <div>
          <strong>Changes applied</strong>
          <span>{result.changes.length} {result.changes.length === 1 ? "file" : "files"}</span>
        </div>
        <button type="button" onClick={() => setExpanded((current) => !current)}>
          {expanded ? "Hide diff" : "Review diff"}
        </button>
      </header>
      <ul className="change-review-files">
        {result.changes.map((change) => (
          <li key={change.relativePath}>
            <span>{change.relativePath}</span>
            <small>{change.created ? "Created" : "Modified"}</small>
            {expanded ? <Diff value={change.diff} /> : null}
          </li>
        ))}
      </ul>
      <footer>
        {undoResult ? (
          <p data-status={undoResult.files.every((file) => file.status === "undone") ? "success" : "warning"}>
            {undoStatus(undoResult)}
          </p>
        ) : canUndo && undoAvailable === null ? (
          <p>Checking whether undo is still safe…</p>
        ) : canUndo && undoAvailable ? (
          <button type="button" onClick={() => void undo()} disabled={disabled || undoing}>
            {undoing ? "Undoing…" : "Undo changes"}
          </button>
        ) : canUndo ? (
          <p>Undo is unavailable because the files changed or access is no longer writable.</p>
        ) : (
          <p>Only the latest applied change can be undone.</p>
        )}
        {error ? <p className="change-review-error" role="alert">{error}</p> : null}
      </footer>
    </section>
  );
}
