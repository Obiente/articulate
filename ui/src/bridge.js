// Transport between the renderer and the desktop controller. In a browser
// preview the same calls reach a synthetic workspace instead. Native
// connection errors never fall back to sample documents.
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { prepareAction } from "./actions.js";
import { createPreviewWorkspace } from "./preview/workspace.js";
import { flatten } from "./state/store.js";

export { sourceMoveAction } from "./actions.js";

export const native = isTauri();
const preview = native ? null : createPreviewWorkspace(globalThis.localStorage);

// Sections newer than `since` ({ section: revision }), or all of them.
export async function fetchState(since) {
  if (native) return invoke("desktop_snapshot", since ? { since } : {});
  return preview.state(since);
}

// The newest section revision; a cheap check for missed events.
export async function fetchRevision() {
  if (native) return invoke("desktop_revision");
  return preview.revision();
}

// Receive changed sections as the controller publishes them.
export async function onState(handler) {
  if (native) return listen("desktop-state", (event) => handler(event.payload));
  return preview.subscribe(handler);
}

// The complete state as one object.
export async function snapshot() {
  return flatten(await fetchState());
}

export async function action(command) {
  const prepared = prepareAction(command);
  if (native) return invoke("desktop_action", { action: prepared });
  return preview.action(prepared);
}

// The close handshake: acknowledge the request, save edits, then close.
export async function onCloseRequested(handler) {
  if (!native) return () => {};
  return listen("desktop-close-requested", (event) => handler(event.payload));
}
export async function acknowledgeClose(id, cancelled = false) {
  if (native) await invoke("desktop_close_ack", { id, cancelled });
}
export async function closeDesktop(force = false) {
  if (native) await invoke("desktop_close", { force });
}
export async function controllerHealth() {
  if (native) return invoke("desktop_health");
  return { state: "running", message: null, can_close_anyway: false };
}

// The dictation indicator window.
export async function overlaySnapshot() {
  return invoke("desktop_overlay");
}
export async function onOverlay(handler) {
  return listen("desktop-overlay", (event) => handler(event.payload));
}
