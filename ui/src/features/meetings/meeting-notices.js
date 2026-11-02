// Meeting detection state for the prompt, the running indicator and Settings.

export const meetingApps = [
  ["teams", "Microsoft Teams", "meetings_teams"],
  ["zoom", "Zoom", "meetings_zoom"],
  ["meet", "Google Meet", "meetings_meet"],
];

// The detected meeting waiting for an answer, or null.
export function meetingPrompt(state) {
  const prompt = state?.meetings?.prompt;
  if (!prompt?.id) return null;
  return {
    id: `meeting-prompt-${prompt.id}`,
    app: prompt.id,
    label: prompt.label || prompt.id,
    bestEffort: Boolean(prompt.best_effort),
    consent: state.meetings.consent_reminder || "",
  };
}

// The meeting whose transcript detection started, while it runs.
export function meetingIndicator(state) {
  const owned = state?.meetings?.owned;
  if (!owned?.id) return null;
  return {
    app: owned.id,
    label: owned.label || owned.id,
    title: `Transcribing the ${owned.label || owned.id} meeting`,
    status: state.meetings.status || "",
  };
}

// Tracks which prompt is on screen so the renderer shows a notification once
// per detected meeting and hides it when the controller clears the prompt.
export function createMeetingPromptTracker() {
  let shown = null;
  return (prompt) => {
    const next = prompt?.id || null;
    if (next === shown) return { show: null, hide: null };
    const hide = shown;
    shown = next;
    return { show: next ? prompt : null, hide };
  };
}

export function formatEventTime(unixMs) {
  if (!Number.isFinite(unixMs) || unixMs <= 0) return "";
  return new Date(unixMs).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
}
