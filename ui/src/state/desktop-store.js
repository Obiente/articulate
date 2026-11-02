// Keeps renderer state in step with the desktop controller.
//
// The controller pushes changed sections as events. After every action and
// whenever a cheap revision check finds a gap (a missed event), the store
// fetches only the sections newer than its own.
import { useEffect, useSyncExternalStore } from "react";
import { applyUpdate, emptyStore } from "./store.js";

export function createDesktopStore(bridge, { pollInterval = 2000 } = {}) {
  let store = emptyStore;
  let error = "";
  let snapshot = { state: null, error: "" };
  const listeners = new Set();
  let inflight = null;
  let again = false;

  function emit() {
    snapshot = { state: store.state, error };
    for (const listener of listeners) listener();
  }

  function setError(message) {
    if (message === error) return;
    error = message;
    emit();
  }

  function apply(update) {
    const { store: next, resync } = applyUpdate(store, update);
    if (next !== store) {
      store = next;
      emit();
    }
    if (resync) void sync().catch(() => {});
  }

  // Fetch sections newer than the local copy. Concurrent calls share one
  // request and repeat it once, so a caller never misses a later change.
  function sync() {
    if (inflight) {
      again = true;
      return inflight;
    }
    inflight = (async () => {
      try {
        do {
          again = false;
          apply(
            await bridge.fetchState(
              store.revision ? store.revisions : undefined,
            ),
          );
          setError("");
        } while (again);
      } catch (e) {
        setError(String(e));
        throw e;
      } finally {
        inflight = null;
      }
    })();
    return inflight;
  }

  // Listen for pushed updates and start the fallback revision check.
  // Returns a function that disconnects; connecting again is safe.
  function connect() {
    let active = true;
    let timer;
    let unlisten;
    const poll = async () => {
      try {
        const revision = await bridge.fetchRevision();
        if (active && revision !== store.revision) await sync();
      } catch (e) {
        if (active) setError(String(e));
      } finally {
        if (active) timer = setTimeout(poll, pollInterval);
      }
    };
    (async () => {
      try {
        const stop = await bridge.onState(apply);
        if (active) unlisten = stop;
        else stop();
      } catch (e) {
        setError(String(e));
      }
      if (!active) return;
      await sync().catch(() => {});
      if (active) timer = setTimeout(poll, pollInterval);
    })();
    return () => {
      active = false;
      clearTimeout(timer);
      unlisten?.();
    };
  }

  return {
    connect,
    sync,
    getSnapshot: () => snapshot,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

// Subscribe a component to the store; returns `{ state, error }`.
export function useDesktopStore(store) {
  useEffect(() => store.connect(), [store]);
  return useSyncExternalStore(store.subscribe, store.getSnapshot);
}
