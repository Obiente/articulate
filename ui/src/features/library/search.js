// Saved-item search: match highlighting, filters and request scheduling.
// Kept free of React so it can be tested directly.

const utf8Width = (codePoint) =>
  codePoint < 0x80 ? 1 : codePoint < 0x800 ? 2 : codePoint < 0x10000 ? 3 : 4;

// Convert UTF-8 byte ranges within `text` to JavaScript string indices.
// Ranges that start or end inside a character widen to whole characters, out
// of range values are clamped, and overlapping ranges are merged.
export function byteRangesToIndices(text, ranges) {
  if (!text || !Array.isArray(ranges) || !ranges.length) return [];
  // Byte offset -> string index, for each character start and the end.
  const starts = [];
  let bytes = 0;
  for (let index = 0; index < text.length; ) {
    const codePoint = text.codePointAt(index);
    starts.push([bytes, index]);
    bytes += utf8Width(codePoint);
    index += codePoint > 0xffff ? 2 : 1;
  }
  starts.push([bytes, text.length]);
  const total = bytes;
  // The last character boundary at or before `offset`.
  const floor = (offset) => {
    let low = 0,
      high = starts.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (starts[middle][0] <= offset) low = middle;
      else high = middle - 1;
    }
    return low;
  };
  const converted = [];
  for (const range of ranges) {
    if (!Array.isArray(range) || range.length < 2) continue;
    let [from, to] = range.map(Number);
    if (!Number.isFinite(from) || !Number.isFinite(to)) continue;
    from = Math.max(0, Math.min(total, from));
    to = Math.max(0, Math.min(total, to));
    if (to <= from) continue;
    const start = starts[floor(from)][1];
    const last = floor(to);
    const end =
      starts[last][0] === to
        ? starts[last][1]
        : starts[Math.min(last + 1, starts.length - 1)][1];
    if (end > start) converted.push([start, end]);
  }
  converted.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged = [];
  for (const range of converted) {
    const previous = merged.at(-1);
    if (previous && range[0] <= previous[1])
      previous[1] = Math.max(previous[1], range[1]);
    else merged.push([...range]);
  }
  return merged;
}

// Split `text` into plain and matched parts for rendering.
export function highlightSegments(text, byteRanges) {
  const ranges = byteRangesToIndices(text || "", byteRanges);
  const segments = [];
  let at = 0;
  for (const [start, end] of ranges) {
    if (start > at)
      segments.push({ text: text.slice(at, start), match: false });
    segments.push({ text: text.slice(start, end), match: true });
    at = end;
  }
  if (at < (text || "").length)
    segments.push({ text: text.slice(at), match: false });
  return segments;
}

export const emptyFilters = Object.freeze({
  from: "",
  to: "",
  kind: null,
  speaker: "",
  app: "",
});

// A local calendar day ("YYYY-MM-DD") as inclusive Unix milliseconds.
function dayBoundary(value, end) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value || "");
  if (!match) return null;
  const date = end
    ? new Date(+match[1], +match[2] - 1, +match[3], 23, 59, 59, 999)
    : new Date(+match[1], +match[2] - 1, +match[3]);
  const time = date.getTime();
  return Number.isFinite(time) && time >= 0 ? time : null;
}

// The `filters` field of a `history_search` action.
export function searchFilters(filters = emptyFilters) {
  const text = (value) => (value || "").trim() || null;
  return {
    from_ms: dayBoundary(filters.from, false),
    to_ms: dayBoundary(filters.to, true),
    kind: filters.kind || null,
    speaker: text(filters.speaker),
    app: text(filters.app),
  };
}

export function sameSearchFilters(left, right) {
  const keys = ["from_ms", "to_ms", "kind", "speaker", "app"];
  return keys.every((key) => (left?.[key] ?? null) === (right?.[key] ?? null));
}

// Filters other than the page's own kind.
export function activeFilterCount(filters, defaultKind) {
  return (
    (filters.from ? 1 : 0) +
    (filters.to ? 1 : 0) +
    ((filters.kind || null) !== (defaultKind || null) ? 1 : 0) +
    (filters.speaker?.trim() ? 1 : 0) +
    (filters.app?.trim() ? 1 : 0)
  );
}

export function filterProblem(filters) {
  const from = dayBoundary(filters.from, false),
    to = dayBoundary(filters.to, true);
  if (from !== null && to !== null && from > to)
    return "Choose a start date before the end date.";
  return null;
}

// What the library should show for the current query and filters, given the
// controller's `search` state. Results for an older query or other filters
// are kept visible but marked stale until the new ones arrive.
export function searchView(search, query, filters) {
  const trimmed = query.trim();
  if (!trimmed) return { active: false };
  const wanted = searchFilters(filters);
  const results = search?.results;
  const current =
    Boolean(results) &&
    results.query === trimmed &&
    search.query === trimmed &&
    sameSearchFilters(search.filters, wanted);
  const settled = current && !search.working;
  return {
    active: true,
    results: results || null,
    stale: !settled,
    error: search?.query === trimmed ? search.error || null : null,
  };
}

// Hits grouped by item, in result order, with at most `perItem` excerpts.
export function groupHits(hits, perItem = 3) {
  const groups = new Map();
  for (const hit of hits || []) {
    let group = groups.get(hit.session_id);
    if (!group) {
      group = { id: hit.session_id, first: hit, hits: [], total: 0 };
      groups.set(hit.session_id, group);
    }
    group.total += 1;
    if (group.hits.length < perItem) group.hits.push(hit);
  }
  return [...groups.values()];
}

export const fieldLabels = Object.freeze({
  title: "Title",
  text: "Text",
  original: "Original wording",
  personal_notes: "Notes",
  generated_notes: "Generated notes",
  transcript: "Transcript",
  speaker_names: "Speakers",
});

// Debounces searches while typing. Clearing the query is sent at once, and
// a search identical to the last one sent is skipped.
export function createSearchScheduler({
  run,
  delay = 250,
  setTimer = setTimeout,
  clearTimer = clearTimeout,
}) {
  let timer = null,
    sent = null;
  const key = (query, filters) => JSON.stringify([query, filters]);
  const send = (query, filters) => {
    timer = null;
    // Filters don't matter while there is nothing to search for.
    const next = query ? key(query, filters) : key("", null);
    if (next === sent) return;
    sent = next;
    run(query, filters);
  };
  return {
    schedule(query, filters) {
      const trimmed = query.trim();
      if (timer !== null) clearTimer(timer);
      timer = null;
      if (!trimmed) send("", filters);
      else timer = setTimer(() => send(trimmed, filters), delay);
    },
    // Forget the last request so the next identical one is sent again.
    reset() {
      sent = null;
    },
    cancel() {
      if (timer !== null) clearTimer(timer);
      timer = null;
    },
  };
}
