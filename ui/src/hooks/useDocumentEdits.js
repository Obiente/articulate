// Debounced document edits (titles and personal notes).
//
// Edits show immediately and save after a short pause. Navigation, document
// actions and closing call `flush` first. A failed save keeps the edit for
// retry. While a document action runs, editing is locked.
import { useCallback, useEffect, useRef, useState } from "react";
import { action } from "../bridge.js";
import { savedPatchValue } from "../actions.js";

const SAVE_DELAY = 650;

export function useDocumentEdits({ state, getState, sync, report }) {
  const pending = useRef(new Map()),
    timer = useRef(null),
    saving = useRef(null),
    transition = useRef(false);
  const [edits, setEdits] = useState({}),
    [savingNow, setSavingNow] = useState(false),
    [documentLocked, setDocumentLocked] = useState(false);

  // Drop local edits once the saved document reports the same values.
  const prune = useCallback(() => {
    const current = getState();
    setEdits((old) => {
      const clean = { ...old };
      for (const s of [
        current?.selected,
        current?.call_session,
        current?.note_session,
      ].filter(Boolean)) {
        if (
          clean[s.id] &&
          !pending.current.has(s.id) &&
          Object.entries(clean[s.id]).every(
            ([k, v]) => s[k] === savedPatchValue(k, v),
          )
        )
          delete clean[s.id];
      }
      return Object.keys(clean).length === Object.keys(old).length
        ? old
        : clean;
    });
  }, [getState]);
  useEffect(prune, [
    state?.selected,
    state?.call_session,
    state?.note_session,
    prune,
  ]);

  const flush = useCallback(async () => {
    clearTimeout(timer.current);
    if (saving.current) return saving.current;
    if (!pending.current.size) return;
    setSavingNow(true);
    const work = (async () => {
      while (pending.current.size) {
        for (const [id, patch] of [...pending.current.entries()]) {
          await action({ type: "session_patch", id, ...patch });
          if (pending.current.get(id) === patch) pending.current.delete(id);
        }
      }
      await sync();
      prune();
    })();
    saving.current = work;
    try {
      await work;
    } finally {
      if (saving.current === work) saving.current = null;
      setSavingNow(false);
    }
  }, [sync, prune]);

  const edit = useCallback(
    (id, patch) => {
      if (transition.current) return;
      pending.current.set(id, { ...(pending.current.get(id) || {}), ...patch });
      setEdits((old) => ({ ...old, [id]: { ...(old[id] || {}), ...patch } }));
      clearTimeout(timer.current);
      timer.current = setTimeout(() => void flush().catch(report), SAVE_DELAY);
    },
    [flush, report],
  );

  // Lock document controls while an action may replace the document.
  const beginTransition = useCallback(() => {
    if (transition.current) return false;
    transition.current = true;
    setDocumentLocked(true);
    return true;
  }, []);
  const endTransition = useCallback(() => {
    transition.current = false;
    setDocumentLocked(false);
  }, []);
  const hasPending = useCallback(() => pending.current.size > 0, []);
  const inTransition = useCallback(() => transition.current, []);

  // Warn before a browser reload loses edits; Ctrl+S saves immediately.
  useEffect(() => {
    const before = (e) => {
      if (pending.current.size) {
        e.preventDefault();
        e.returnValue = "";
      }
    };
    const key = (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "s") {
        e.preventDefault();
        void flush().catch(report);
      }
    };
    window.addEventListener("beforeunload", before);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("beforeunload", before);
      window.removeEventListener("keydown", key);
    };
  }, [flush, report]);

  return {
    edits,
    savingNow,
    documentLocked,
    hasPending,
    inTransition,
    flush,
    edit,
    beginTransition,
    endTransition,
  };
}
