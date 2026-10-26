import type { CatalogueModule } from "../../generated/ipc";

export function moduleSetupReadiness(modules: CatalogueModule[]) {
  const required = modules.filter((module) => module.required);
  return {
    requiredReady: required.length > 0 && required.every((module) => module.registrationState === "available"),
    allReady: modules.length > 0 && modules.every((module) => module.registrationState === "available"),
  };
}

export function moduleStatusLabel(module: CatalogueModule): string {
  switch (module.registrationState) {
    case "available": return module.source === "imported" ? "Imported · Verified" : "Downloaded · Verified";
    case "unavailable": return "Unavailable — locate file";
    case "verificationFailed": return "File changed — verify again";
    default: return "Not added";
  }
}
