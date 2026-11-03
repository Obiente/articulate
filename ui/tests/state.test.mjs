import test from "node:test";
import assert from "node:assert/strict";
import {
  applyUpdate,
  emptyStore,
  flatten,
  reconcile,
} from "../src/state/store.js";
import {
  actionCatalog,
  changesDocument,
  prepareAction,
  previewSupports,
} from "../src/actions.js";
import { createDesktopStore } from "../src/state/desktop-store.js";

const row = (start_ms, text, extra = {}) => ({
  start_ms,
  end_ms: start_ms + 1000,
  speaker: "You",
  speaker_labels: ["You"],
  text,
  cues: [],
  provisional: false,
  ...extra,
});

test("sections merge into one state and unchanged values keep their identity", () => {
  const first = applyUpdate(emptyStore, {
    revision: 2,
    sections: {
      shell: { revision: 1, data: { ready: false, status: "Starting" } },
      history: {
        revision: 2,
        data: {
          history: [
            { id: "a", title: "A" },
            { id: "b", title: "B" },
          ],
        },
      },
    },
  }).store;
  assert.equal(first.state.status, "Starting");
  assert.deepEqual(first.state.call_rows, []);
  const history = first.state.history;
  const second = applyUpdate(first, {
    revision: 4,
    sections: {
      shell: { revision: 3, data: { ready: true, status: "Ready" } },
      history: {
        revision: 4,
        data: {
          history: [
            { id: "c", title: "C" },
            { id: "a", title: "A" },
            { id: "b", title: "B" },
          ],
        },
      },
    },
  }).store;
  assert.equal(second.state.ready, true);
  assert.notEqual(second.state.history, history);
  assert.equal(second.state.history[1], history[0], "matched by id");
  assert.equal(second.state.history[2], history[1]);
  const stale = applyUpdate(second, {
    revision: 3,
    sections: { shell: { revision: 3, data: { ready: false } } },
  });
  assert.equal(stale.store, second, "older or repeated sections are ignored");
});

test("row splices extend the previous revision and request a resync on gaps", () => {
  const start = applyUpdate(emptyStore, {
    revision: 1,
    sections: {
      live_rows: {
        revision: 1,
        base: 0,
        splices: {
          call_rows: { start: 0, rows: [row(0, "One."), row(1000, "Two.")] },
          note_rows: { start: 0, rows: [] },
        },
      },
    },
  }).store;
  const firstRow = start.state.call_rows[0];
  const next = applyUpdate(start, {
    revision: 2,
    sections: {
      live_rows: {
        revision: 2,
        base: 1,
        splices: {
          call_rows: {
            start: 1,
            rows: [row(1000, "Two, revised."), row(2000, "Three.")],
          },
        },
      },
    },
  });
  assert.equal(next.resync, false);
  const rows = next.store.state.call_rows;
  assert.deepEqual(
    rows.map((r) => r.text),
    ["One.", "Two, revised.", "Three."],
  );
  assert.equal(rows[0], firstRow, "earlier rows keep their identity");
  const gap = applyUpdate(next.store, {
    revision: 9,
    sections: {
      live_rows: {
        revision: 9,
        base: 7,
        splices: { call_rows: { start: 0, rows: [] } },
      },
    },
  });
  assert.equal(gap.resync, true);
  assert.equal(gap.store.state.call_rows, rows, "a gap changes nothing");
  const full = applyUpdate(gap.store, {
    revision: 9,
    sections: {
      live_rows: {
        revision: 9,
        data: { call_rows: [row(0, "One.")], note_rows: [] },
      },
    },
  }).store;
  assert.equal(full.state.call_rows[0], firstRow);
  assert.equal(full.state.call_rows.length, 1);
});

test("the open document joins its separately sent body", () => {
  const meta = { id: "doc", title: "Doc", personal_notes: "Notes" };
  const state = flatten({
    revision: 2,
    sections: {
      document: { revision: 1, data: { selected: meta } },
      document_body: {
        revision: 2,
        data: {
          selected_body: {
            id: "doc",
            text: "Body",
            original: "",
            rows: [row(0, "Hi.")],
          },
        },
      },
    },
  });
  assert.equal(state.selected.text, "Body");
  assert.equal(state.selected.personal_notes, "Notes");
  assert.equal(state.selected.rows[0].text, "Hi.");
  const loading = flatten({
    revision: 2,
    sections: {
      document: { revision: 1, data: { selected: meta } },
      document_body: { revision: 2, data: { selected_body: null } },
    },
  });
  assert.equal(loading.selected.text, "");
  assert.deepEqual(loading.selected.rows, []);
  const store = applyUpdate(emptyStore, {
    revision: 2,
    sections: {
      document: { revision: 1, data: { selected: meta } },
      document_body: {
        revision: 2,
        data: {
          selected_body: { id: "doc", text: "Body", original: "", rows: [] },
        },
      },
    },
  }).store;
  const edited = applyUpdate(store, {
    revision: 3,
    sections: {
      document: {
        revision: 3,
        data: { selected: { ...meta, personal_notes: "Edited" } },
      },
    },
  }).store;
  assert.equal(edited.state.selected.personal_notes, "Edited");
  assert.equal(edited.state.selected.rows, store.state.selected.rows);
});

test("reconcile reuses equal nested values", () => {
  const previous = { a: { b: [1, 2] }, c: "x" };
  assert.equal(reconcile(previous, { a: { b: [1, 2] }, c: "x" }), previous);
  const changed = reconcile(previous, { a: { b: [1, 2] }, c: "y" });
  assert.notEqual(changed, previous);
  assert.equal(changed.a, previous.a);
});

test("the action catalog flags document actions and preview support", () => {
  for (const type of [
    "history_open",
    "note_new",
    "dictation_start",
    "call_stop",
    "update_install",
  ])
    assert.equal(changesDocument(type), true, type);
  for (const type of ["copy", "settings_patch", "session_patch"])
    assert.equal(changesDocument(type), false, type);
  assert.equal(previewSupports("call_start"), false);
  assert.equal(previewSupports("session_patch"), true);
  assert.ok(Object.isFrozen(actionCatalog));
});

test("shared validation rejects invalid actions before either transport", () => {
  assert.throws(() => prepareAction({ type: "run_shell" }), /not available/);
  assert.throws(
    () => prepareAction({ type: "session_patch", id: "a", title: "\u0007" }),
    /title from 1 to 160/,
  );
  assert.throws(
    () => prepareAction({ type: "export", text: "x", extension: "exe" }),
    /Unsupported export/,
  );
  assert.throws(
    () => prepareAction({ type: "dictation_edit", id: "a", text: "  " }),
    /Enter some text/,
  );
  const move = {
    type: "source_move",
    id: "a",
    topic_id: "b",
    expected_topic_id: null,
  };
  assert.equal(prepareAction(move), move);
});

test("the desktop store applies pushed updates and catches up after a gap", async () => {
  let push;
  let revision = 1;
  const requests = [];
  const sections = (rev) => ({
    revision: rev,
    sections: { shell: { revision: rev, data: { status: `rev ${rev}` } } },
  });
  const store = createDesktopStore(
    {
      async onState(handler) {
        push = handler;
        return () => {};
      },
      async fetchState(since) {
        requests.push(since);
        return sections(revision);
      },
      async fetchRevision() {
        return revision;
      },
    },
    { pollInterval: 5 },
  );
  const disconnect = store.connect();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(store.getSnapshot().state.status, "rev 1");
  push(sections(2));
  assert.equal(store.getSnapshot().state.status, "rev 2");
  revision = 5;
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(store.getSnapshot().state.status, "rev 5");
  assert.deepEqual(requests.at(-1), { shell: 2 });
  disconnect();
});
