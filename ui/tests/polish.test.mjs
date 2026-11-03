import test from "node:test";
import assert from "node:assert/strict";
import { prepareAction } from "../src/actions.js";

test("automatic cleanup preference accepts only booleans", () => {
  for (const auto_polish of [true, false]) {
    assert.equal(prepareAction({ type: "settings_patch", auto_polish }).auto_polish, auto_polish);
  }
  for (const auto_polish of ["true", 1, null]) {
    assert.throws(() => prepareAction({ type: "settings_patch", auto_polish }), /on or off/);
  }
});
