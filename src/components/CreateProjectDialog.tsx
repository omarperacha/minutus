import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";

type CreateProjectDialogProps = {
  pending: boolean;
  error: string | null;
  onCancel: () => void;
  onCreate: (name: string) => void;
};

export function CreateProjectDialog({
  pending,
  error,
  onCancel,
  onCreate,
}: CreateProjectDialogProps) {
  const [name, setName] = useState("");
  const input = useRef<HTMLInputElement | null>(null);
  const trimmedName = name.trim();

  useEffect(() => {
    input.current?.focus();
  }, []);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !pending) onCancel();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onCancel, pending]);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (trimmedName.length > 0 && !pending) onCreate(trimmedName);
  }

  return (
    <div className="project-dialog-backdrop">
      <section
        className="project-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-project-title"
      >
        <p className="eyebrow">New project</p>
        <h2 id="create-project-title">What should this project be called?</h2>
        <p>Keep related chats together. Folder access is optional and can be added later in Project settings.</p>
        <form onSubmit={submit}>
          <label htmlFor="project-name">Project name</label>
          <input
            ref={input}
            id="project-name"
            value={name}
            onChange={(event) => setName(event.target.value)}
            maxLength={160}
            autoComplete="off"
            placeholder="e.g. Product research"
            disabled={pending}
          />
          {error ? <p className="project-dialog-error" role="alert">{error}</p> : null}
          <div className="project-dialog-actions">
            <button type="button" onClick={onCancel} disabled={pending}>Cancel</button>
            <button type="submit" disabled={pending || trimmedName.length === 0}>
              {pending ? "Creating…" : "Create project"}
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}
