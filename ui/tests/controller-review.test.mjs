import test from "node:test";
import assert from "node:assert/strict";
import { validTitle } from "../src/actions.js";
import { flushPatches } from "../src/hooks/document-edits.js";
import { createDesktopStore } from "../src/state/desktop-store.js";
import { applyUpdate, emptyStore } from "../src/state/store.js";

test("title drafts reject empty, oversized and control characters", () => {
  for (const title of ["", "  ", "x".repeat(161), "bad\0title"])
    assert.equal(validTitle(title), false);
  assert.equal(validTitle(" A title "), true);
});
test("flush reports invalid patches once and continues saving", async () => {
  const pending = new Map([
    ["bad", { title: "" }],
    ["good", { title: "Valid" }],
  ]);
  const sent = [],
    errors = [];
  await flushPatches(
    pending,
    async (a) => sent.push(a),
    (...e) => errors.push(e),
  );
  await flushPatches(
    pending,
    async (a) => sent.push(a),
    (...e) => errors.push(e),
  );
  assert.equal(errors.length, 1);
  assert.equal(sent.length, 1);
  assert.equal(pending.size, 0);
});
test("flush retains transport failures and drains concurrent replacements", async () => {
  const pending = new Map([["doc", { title: "First" }]]);
  await assert.rejects(
    flushPatches(
      pending,
      async () => {
        throw Error("disk");
      },
      assert.fail,
    ),
    /disk/,
  );
  assert.equal(pending.size, 1);
  const sent = [];
  await flushPatches(
    pending,
    async (a) => {
      sent.push(a.title);
      if (sent.length === 1) pending.set("doc", { title: "Second" });
    },
    assert.fail,
  );
  assert.deepEqual(sent, ["First", "Second"]);
  assert.equal(pending.size, 0);
});
test("fallback detects a missed older section after a newer event", async () => {
  let push,
    missed = false;
  const store = createDesktopStore(
    {
      async onState(fn) {
        push = fn;
        return () => {};
      },
      async fetchState() {
        return {
          sections: {
            older: {
              revision: missed ? 2 : 1,
              data: { text: missed ? "new" : "old" },
            },
          },
        };
      },
      async fetchRevision() {
        return { older: 2, latest: 3 };
      },
    },
    { pollInterval: 5 },
  );
  const stop = store.connect();
  try {
    await store.sync();
    missed = true;
    push({ sections: { latest: { revision: 3, data: { status: "latest" } } } });
    await new Promise((resolve) => setTimeout(resolve, 40));
    assert.equal(store.getSnapshot().state.text, "new");
  } finally {
    stop();
  }
});
test("body composition ignores mismatched documents and preserves identity", () => {
  let store = applyUpdate(emptyStore, {
    sections: {
      dictation: {
        revision: 1,
        data: { dictation_session: { id: "new", title: "Title" } },
      },
      capture_bodies: {
        revision: 2,
        data: {
          dictation_body: { id: "old", personal_notes: "private", text: "old" },
        },
      },
    },
  }).store;
  assert.equal(store.state.dictation_session.text, "");
  assert.equal(store.state.dictation_session.personal_notes, "");
  store = applyUpdate(store, {
    sections: {
      capture_bodies: {
        revision: 3,
        data: {
          dictation_body: {
            id: "new",
            text: "body",
            personal_notes: "notes",
            rows: [],
          },
        },
      },
    },
  }).store;
  assert.equal(store.state.dictation_session.personal_notes, "notes");
  const next = applyUpdate(store, {
    sections: { shell: { revision: 4, data: { status: "ready" } } },
  }).store;
  assert.equal(next.state.dictation_session, store.state.dictation_session);
});
