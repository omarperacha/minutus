import assert from "node:assert/strict";
import test from "node:test";
import { modelFileErrorMessage } from "./modelFileErrors.ts";

test("permission failures retain the native rejection instead of a generic import error", () => {
  const rejected = "register_external_model not allowed. Permissions associated with this command: allow-register-external-model";
  assert.equal(modelFileErrorMessage(rejected), rejected);
  assert.equal(modelFileErrorMessage({ code: "invalidRequest", message: "This file does not match the catalogue model." }), "This file does not match the catalogue model.");
  assert.equal(modelFileErrorMessage(new Error("Event subscription failed")), "Event subscription failed");
  assert.equal(modelFileErrorMessage(null), "Could not register these model files. Please try again.");
});
