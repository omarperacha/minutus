import assert from "node:assert/strict";
import test from "node:test";
import { modelAvailabilityLabel, localFileActionLabel } from "./availability.ts";

test("first presentation offers import while missing registered files offer locate", () => {
  assert.equal(modelAvailabilityLabel({ registrationState: "notInstalled" }), "Not installed");
  assert.equal(localFileActionLabel({ registrationState: "notInstalled", externalFiles: false }), "Import from local files");
  assert.equal(modelAvailabilityLabel({ registrationState: "unavailable" }), "Unavailable — file missing or inaccessible");
  assert.equal(localFileActionLabel({ registrationState: "unavailable", externalFiles: true }), "Locate files…");
});

test("changed files offer recovery while available models do not offer import", () => {
  assert.equal(modelAvailabilityLabel({ registrationState: "verificationFailed" }), "Verification failed — files changed");
  assert.equal(localFileActionLabel({ registrationState: "verificationFailed", externalFiles: true }), "Locate files…");
  assert.equal(modelAvailabilityLabel({ registrationState: "available" }), "Available");
  assert.equal(localFileActionLabel({ registrationState: "available", externalFiles: true }), null);
  assert.equal(localFileActionLabel({ registrationState: "available", externalFiles: false }), null);
});
