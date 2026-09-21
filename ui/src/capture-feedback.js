// Observe native lifecycle acknowledgements rather than requested button actions.
// Each renderer starts with a baseline so reopening it never replays an old cue.
export function createCaptureFeedbackTracker() {
  let lastSequence = null;
  return (feedback) => {
    const sequence = feedback?.sequence;
    if (!Number.isSafeInteger(sequence) || sequence < 0) return null;
    if (lastSequence === null) {
      lastSequence = sequence;
      return null;
    }
    if (sequence <= lastSequence) return null;
    lastSequence = sequence;
    if (
      !["call", "note"].includes(feedback.kind) ||
      !["started", "stopped", "failed"].includes(feedback.event)
    )
      return null;
    // Pre-capture failures already surface through the command error (or the
    // native automatic-capture notice). Only report ongoing session failures.
    if (feedback.event === "failed" && !feedback.session_id) return null;
    const label = feedback.kind === "call" ? "Call recording" : "Spoken note";
    const title =
      feedback.event === "started"
        ? `${label} started`
        : feedback.event === "stopped"
          ? `${label} stopped`
          : `${label} needs attention`;
    const message =
      feedback.event === "started"
        ? "Your words are being captured in Notetaker."
        : feedback.event === "stopped"
          ? "Recording has stopped. Finishing the transcript in Notetaker."
          : feedback.status || "Open Notetaker to check the recording.";
    return {
      sequence,
      event: feedback.event,
      title,
      message,
      sessionId: feedback.session_id || null,
    };
  };
}
