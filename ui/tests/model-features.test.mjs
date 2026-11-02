import test from "node:test";
import assert from "node:assert/strict";
import { textDiff } from "../src/features/polish/diff.js";
import { shouldCheckUpdates } from "../src/features/updates/update-policy.js";
test("diff reconstructs both versions without splitting Unicode", () => {
 for (const [a,b] of [["Hello world", "Hello, world!"], ["", "hello"], ["same", "same"], ["🙂 old", "🙂 new"]]) { const d = textDiff(a,b); assert.equal(d.prefix+d.removed+d.suffix,a); assert.equal(d.prefix+d.added+d.suffix,b); }
});
test("automatic checks require consent and an idle updater", () => { for (const state of ["idle", "latest", "error", "checking", "downloading", "ready", "available"]) { assert.equal(shouldCheckUpdates(false,state),false); assert.equal(shouldCheckUpdates(undefined,state),false); assert.equal(shouldCheckUpdates(true,state),["idle","latest","error"].includes(state)); } });
