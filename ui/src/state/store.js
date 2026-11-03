// Applies desktop state updates to renderer state.
//
// The controller publishes named sections, each with a revision. A section's
// data is an object whose keys merge into one flat state object, so pages
// read `state.history`, `state.settings` and so on. Unchanged values keep
// their previous object identity, so memoized components skip work.
// See docs/desktop-ui-contract.md.

const EMPTY_ROWS = Object.freeze([]);

export const emptyStore = Object.freeze({
  revision: 0,
  revisions: Object.freeze({}),
  values: Object.freeze({}),
  state: null,
});

const isObject = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value);

// Return `next`, reusing `previous` (or any of its parts) where they are equal.
export function reconcile(previous, next) {
  if (Object.is(previous, next)) return previous;
  if (Array.isArray(next)) {
    if (!Array.isArray(previous)) return next;
    const byId =
      next.length > 0 && next.every((item) => typeof item?.id === "string")
        ? new Map(previous.map((item) => [item?.id, item]))
        : null;
    let same = previous.length === next.length;
    const result = next.map((item, index) => {
      const value = reconcile(byId ? byId.get(item.id) : previous[index], item);
      if (value !== previous[index]) same = false;
      return value;
    });
    return same ? previous : result;
  }
  if (isObject(next)) {
    if (!isObject(previous)) return next;
    const keys = Object.keys(next);
    let same = keys.length === Object.keys(previous).length;
    const result = {};
    for (const key of keys) {
      const value = reconcile(previous[key], next[key]);
      if (value !== previous[key] || !Object.hasOwn(previous, key))
        same = false;
      result[key] = value;
    }
    return same ? previous : result;
  }
  return next;
}

function splice(rows = EMPTY_ROWS, { start, rows: replacement }) {
  if (start > rows.length) return null;
  const next = rows.slice(0, start);
  replacement.forEach((row, offset) =>
    next.push(reconcile(rows[start + offset], row)),
  );
  return next.length === rows.length &&
    next.every((row, index) => row === rows[index])
    ? rows
    : next;
}

// The open document arrives as metadata (`selected`) and a separately sent
// body (`selected_body`), so note edits never resend a long transcript.
function derive(values, previous) {
  const state = { call_rows: EMPTY_ROWS, note_rows: EMPTY_ROWS, ...values };
  for (const [key, bodyKey] of [
    ["selected", "selected_body"],
    ["dictation_session", "dictation_body"],
    ["call_session", "call_body"],
    ["note_session", "note_body"],
  ]) {
    const meta = values[key];
    if (!meta || !(bodyKey in values)) continue;
    const body = values[bodyKey]?.id === meta.id ? values[bodyKey] : null;
    state[key] = reconcile(previous?.state?.[key], {
      ...meta,
      text: body?.text ?? "",
      original: body?.original ?? "",
      personal_notes: body?.personal_notes ?? meta.personal_notes ?? "",
      rows: body?.rows ?? EMPTY_ROWS,
    });
  }
  return { state };
}

// Apply an update. Returns the next store and whether a complete resync is
// needed because row splices did not match the local revision.
export function applyUpdate(store, update) {
  let values = store.values;
  let revisions = store.revisions;
  let resync = false;
  for (const [name, section] of Object.entries(update?.sections ?? {})) {
    const known = revisions[name] ?? 0;
    if (section.revision <= known) continue;
    if (section.splices) {
      if (section.base !== known) {
        resync = true;
        continue;
      }
      const changed = {};
      let mismatched = false;
      for (const [key, rows] of Object.entries(section.splices)) {
        const next = splice(values[key], rows);
        if (!next) {
          mismatched = true;
          break;
        }
        changed[key] = next;
      }
      if (mismatched) {
        resync = true;
        continue;
      }
      values = { ...values, ...changed };
    } else {
      const changed = {};
      for (const [key, value] of Object.entries(section.data ?? {}))
        changed[key] = reconcile(values[key], value);
      values = { ...values, ...changed };
    }
    revisions = { ...revisions, [name]: section.revision };
  }
  if (values === store.values && revisions === store.revisions)
    return { store, resync };
  const revision = Math.max(store.revision, ...Object.values(revisions));
  const derived = derive(values, store);
  return {
    store: {
      revision,
      revisions,
      values,
      state: derived.state,
      composed: derived.composed,
    },
    resync,
  };
}

// Merge a complete update into one flat object, as the renderer sees it.
export function flatten(update) {
  return applyUpdate(emptyStore, update).store.state;
}
