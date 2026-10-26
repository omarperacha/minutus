import type { CatalogueModel } from "../../generated/ipc";

export function modelAvailabilityLabel(model: Pick<CatalogueModel, "registrationState">): string {
  switch (model.registrationState) {
    case "incomplete": return "Setup incomplete — required modules missing";
    case "available": return "Available";
    case "unavailable": return "Unavailable — file missing or inaccessible";
    case "verificationFailed": return "Verification failed — files changed";
    default: return "Not installed";
  }
}

export function localFileActionLabel(model: Pick<CatalogueModel, "registrationState">): string | null {
  if (model.registrationState === "available") return null;
  return model.registrationState === "unavailable" || model.registrationState === "verificationFailed"
    ? "Locate files…" : "Import from local files";
}
