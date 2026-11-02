import test from "node:test";
import assert from "node:assert/strict";
import {
  activeFilterCount,
  byteRangesToIndices,
  createSearchScheduler,
  emptyFilters,
  filterProblem,
  groupHits,
  highlightSegments,
  searchFilters,
  searchView,
} from "../src/features/library/search.js";
import { pinnedFirst } from "../src/features/library/pins.js";
import { attentionCount } from "../src/features/library/attention.js";
import {
  policyProblem,
  retentionStep,
} from "../src/features/library/retention.js";
import {
  defaultReminder,
  reminderProblem,
  reminderTime,
} from "../src/features/notes/exports.js";
import { searchRecords } from "../src/preview/library.js";

const utf8 = (text) => new TextEncoder().encode(text).length;
const marked = (text, ranges) =>
  highlightSegments(text, ranges)
    .filter((segment) => segment.match)
    .map((segment) => segment.text);

test("byte ranges map to string indices for ASCII text", () => {
  assert.deepEqual(byteRangesToIndices("plan the release", [[9, 16]]), [
    [9, 16],
  ]);
  assert.deepEqual(marked("plan the release", [[0, 4]]), ["plan"]);
});

test("byte ranges account for multi-byte and astral characters", () => {
  const text = "Café ☕ review 🎉 notes";
  const start = utf8("Café ☕ ");
  assert.deepEqual(marked(text, [[start, start + 6]]), ["review"]);
  const emoji = utf8("Café ☕ review ");
  assert.deepEqual(marked(text, [[emoji, emoji + 4]]), ["🎉"]);
  assert.deepEqual(marked(text, [[0, 5]]), ["Café"]);
  // The JavaScript indices differ from the byte offsets after "é".
  assert.deepEqual(byteRangesToIndices(text, [[start, start + 6]]), [
    [7, 13],
  ]);
});

test("ranges inside a character widen to whole characters", () => {
  const text = "naïve";
  // "ï" is two bytes at offsets 2..4; a range ending inside it includes it.
  assert.deepEqual(marked(text, [[0, 3]]), ["naï"]);
  assert.deepEqual(marked(text, [[3, 5]]), ["ïv"]);
  const emoji = "a🎉b";
  assert.deepEqual(marked(emoji, [[2, 3]]), ["🎉"]);
});

test("invalid, empty, out-of-range and overlapping ranges are handled", () => {
  const text = "overlapping words";
  assert.deepEqual(byteRangesToIndices(text, null), []);
  assert.deepEqual(byteRangesToIndices("", [[0, 3]]), []);
  assert.deepEqual(
    byteRangesToIndices(text, [
      [5, 5],
      [9, 4],
      ["x", 3],
      [3],
    ]),
    [],
  );
  assert.deepEqual(byteRangesToIndices(text, [[12, 500]]), [[12, 17]]);
  assert.deepEqual(
    byteRangesToIndices(text, [
      [4, 8],
      [0, 5],
      [12, 17],
    ]),
    [
      [0, 8],
      [12, 17],
    ],
  );
  const segments = highlightSegments(text, [[4, 8]]);
  assert.equal(segments.map((segment) => segment.text).join(""), text);
});

test("preview search hits highlight the matched words", () => {
  const records = [
    {
      id: "a",
      title: "Résumé review",
      kind: "note",
      created_ms: 1,
      updated_ms: 2,
      personal_notes: "Préparer la révision. Révision finale vendredi.",
      text: "",
      rows: [],
    },
  ];
  const hits = searchRecords(records, "révision");
  assert.equal(hits.length, 1);
  assert.deepEqual(marked(hits[0].excerpt, hits[0].matches), [
    "révision",
    "Révision",
  ]);
  assert.equal(searchRecords(records, "révision", { kind: "call" }).length, 0);
});

test("search filters convert local days to inclusive milliseconds", () => {
  const filters = searchFilters({
    ...emptyFilters,
    from: "2026-03-01",
    to: "2026-03-02",
    kind: "call",
    speaker: "  Casey ",
    app: " ",
  });
  assert.equal(filters.from_ms, new Date(2026, 2, 1).getTime());
  assert.equal(filters.to_ms, new Date(2026, 2, 2, 23, 59, 59, 999).getTime());
  assert.equal(filters.kind, "call");
  assert.equal(filters.speaker, "Casey");
  assert.equal(filters.app, null);
  assert.equal(searchFilters(emptyFilters).from_ms, null);
  assert.equal(
    filterProblem({ ...emptyFilters, from: "2026-03-02", to: "2026-03-01" }),
    "Choose a start date before the end date.",
  );
  assert.equal(filterProblem({ ...emptyFilters, from: "2026-03-02" }), null);
  assert.equal(
    activeFilterCount({ ...emptyFilters, kind: "note" }, "note"),
    0,
  );
  assert.equal(
    activeFilterCount({ ...emptyFilters, kind: null, speaker: "x" }, "note"),
    2,
  );
});

test("search results stay marked stale until they match query and filters", () => {
  const filters = { ...emptyFilters, kind: "note" };
  const wanted = searchFilters(filters);
  const results = { query: "plan", hits: [] };
  assert.deepEqual(searchView(null, "  ", filters), { active: false });
  let view = searchView(
    { query: "plan", filters: wanted, working: false, results },
    "plan",
    filters,
  );
  assert.equal(view.stale, false);
  view = searchView(
    { query: "plan", filters: wanted, working: true, results },
    "plan",
    filters,
  );
  assert.equal(view.stale, true);
  view = searchView(
    { query: "pla", filters: wanted, working: false, results },
    "plan",
    filters,
  );
  assert.equal(view.stale, true);
  assert.equal(view.results, results);
  view = searchView(
    { query: "plan", filters: { ...wanted, kind: "call" }, results },
    "plan",
    filters,
  );
  assert.equal(view.stale, true);
  view = searchView(
    { query: "plan", filters: wanted, error: "Bad query" },
    "plan",
    filters,
  );
  assert.equal(view.error, "Bad query");
  view = searchView(
    { query: "old", filters: wanted, error: "Bad query" },
    "plan",
    filters,
  );
  assert.equal(view.error, null);
});

test("the search scheduler debounces typing and skips repeated requests", () => {
  const timers = new Map();
  let next = 1;
  const sent = [];
  const scheduler = createSearchScheduler({
    delay: 250,
    run: (query, filters) => sent.push([query, filters]),
    setTimer: (callback) => {
      timers.set(next, callback);
      return next++;
    },
    clearTimer: (id) => timers.delete(id),
  });
  const flush = () => {
    const pending = [...timers.values()];
    timers.clear();
    pending.forEach((callback) => callback());
  };
  const filters = { kind: "note" };
  scheduler.schedule("p", filters);
  scheduler.schedule("pl", filters);
  scheduler.schedule(" plan ", filters);
  assert.equal(timers.size, 1);
  flush();
  assert.deepEqual(sent, [["plan", filters]]);
  scheduler.schedule("plan", filters);
  flush();
  assert.equal(sent.length, 1, "an identical search is not repeated");
  scheduler.schedule("plan", { kind: "call" });
  flush();
  assert.equal(sent.length, 2, "changed filters search again");
  scheduler.schedule("plan again", filters);
  scheduler.schedule("", filters);
  assert.equal(timers.size, 0, "clearing cancels the pending search");
  assert.deepEqual(sent.at(-1), ["", filters]);
  scheduler.schedule("", { kind: "call" });
  assert.equal(sent.length, 3, "filters alone don't resend an empty query");
  scheduler.reset();
  scheduler.schedule("", filters);
  assert.equal(sent.length, 4);
  scheduler.schedule("later", filters);
  scheduler.cancel();
  assert.equal(timers.size, 0);
});

test("hits group by item in result order", () => {
  const hit = (id, n) => ({ session_id: id, excerpt: `${id}${n}` });
  const groups = groupHits(
    [hit("a", 1), hit("b", 1), hit("a", 2), hit("a", 3), hit("a", 4)],
    3,
  );
  assert.deepEqual(
    groups.map((group) => [group.id, group.hits.length, group.total]),
    [
      ["a", 3, 4],
      ["b", 1, 1],
    ],
  );
  assert.deepEqual(groupHits(undefined), []);
});

test("pinned items come first without reordering the rest", () => {
  const items = [
    { id: "a" },
    { id: "b", pinned: true },
    { id: "c" },
    { id: "d", pinned: true },
  ];
  assert.deepEqual(
    pinnedFirst(items).map((item) => item.id),
    ["b", "d", "a", "c"],
  );
  const unpinned = [{ id: "a" }, { id: "b" }];
  assert.equal(pinnedFirst(unpinned), unpinned);
});

test("attention counts isolated files and other unreadable records", () => {
  assert.equal(attentionCount(null), 0);
  assert.equal(
    attentionCount({
      quarantined: 2,
      warnings: [
        { kind: "damaged" },
        { kind: "unavailable" },
        { kind: "newer_version" },
      ],
    }),
    4,
  );
});

test("turning retention on requires a preview of the same policy", () => {
  const saved = {
    enabled: false,
    days: 90,
    dictations: true,
    calls: true,
    notes: true,
  };
  assert.equal(retentionStep(saved, { ...saved }, null), "saved");
  const draft = { ...saved, enabled: true };
  assert.equal(retentionStep(saved, draft, null), "preview");
  assert.equal(
    retentionStep(saved, draft, { policy: { ...draft, days: 30 } }),
    "preview",
  );
  assert.equal(retentionStep(saved, draft, { policy: { ...draft } }), "confirm");
  assert.equal(
    retentionStep(saved, draft, { policy: { ...draft }, error: "Failed" }),
    "preview",
  );
  assert.equal(
    retentionStep(draft, { ...draft, enabled: false }, null),
    "save",
  );
  assert.equal(retentionStep(saved, { ...draft, days: 0 }, null), "invalid");
  assert.match(policyProblem({ ...draft, days: 1.5 }), /between 1 and/);
  assert.match(
    policyProblem({ ...draft, dictations: false, calls: false, notes: false }),
    /at least one/,
  );
});

test("follow-up reminders use local time and must be in the future", () => {
  const now = new Date(2026, 4, 10, 15, 30);
  assert.equal(defaultReminder(now), "2026-05-11T09:00");
  assert.equal(
    reminderTime("2026-05-11T09:00"),
    new Date(2026, 4, 11, 9).getTime(),
  );
  assert.equal(reminderTime("tomorrow"), null);
  assert.equal(
    reminderProblem("2026-05-10T15:00", now.getTime()),
    "Choose a time in the future.",
  );
  assert.equal(reminderProblem("", now.getTime()), "Choose a date and time.");
  assert.equal(reminderProblem("2026-05-11T09:00", now.getTime()), null);
});
