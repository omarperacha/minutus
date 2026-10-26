/** Preserve Tauri's string ACL/transport errors as well as structured native errors. */
export function modelFileErrorMessage(failure: unknown): string {
  if (typeof failure === "string" && failure.trim()) return failure;
  if (typeof failure === "object" && failure !== null && "message" in failure
    && typeof failure.message === "string" && failure.message.trim()) return failure.message;
  return "Could not register these model files. Please try again.";
}
