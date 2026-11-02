import { Switch } from "@mantine/core";

// Shared state and helpers for every settings section.
export function settingsControls({
  state,
  act,
  pending,
  setPending,
  editShortcut,
  confirm,
}) {
  const settings = state.settings || {};
  const notes = state.notes || {};
  const models = state.model_status || {};
  const capturing =
    state.recording || state.call_recording || state.note_recording;
  const modelBusy =
    capturing ||
    state.busy ||
    state.loading ||
    state.summary_working ||
    state.filing_working ||
    models.cleanup_working ||
    models.speakers_working ||
    models.verifier_working ||
    models.context_progress != null ||
    notes.downloading ||
    state.downloading != null;
  // Run one request at a time; `pending` names it for loading indicators.
  const run = async (name, command) => {
    setPending(name);
    try {
      await act(command);
      return true;
    } catch {
      /* Root presents the error. */
      return false;
    } finally {
      setPending("");
    }
  };
  const patch = (name, value) =>
    run(name, { type: "settings_patch", [name]: value });
  const action = (name, payload = {}) => run(name, { type: name, ...payload });
  const toggle = (name, label) => (
    <Switch
      aria-label={label}
      size="md"
      checked={Boolean(settings[name])}
      disabled={Boolean(pending)}
      onChange={(event) => patch(name, event.currentTarget.checked)}
    />
  );
  return {
    state,
    settings,
    notes,
    models,
    capturing,
    modelBusy,
    pending,
    busy: Boolean(pending),
    patch,
    action,
    toggle,
    editShortcut,
    confirm,
  };
}
