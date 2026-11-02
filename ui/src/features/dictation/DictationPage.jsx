import { useMemo, useState } from "react";
import { Alert, Badge, Button, Group, Kbd } from "@mantine/core";
import { ArrowUpRight, Copy, Microphone, Stop } from "@phosphor-icons/react";
import { formatClock, formatTime } from "../../lib/format.js";
import { TranscriptEditor } from "./TranscriptEditor.jsx";

export function DictationPage({ state, act, open, settings, binding }) {
  const [inserting, setInserting] = useState(false);
  const recent = useMemo(
    () => state.history.filter((s) => s.kind === "dictation").slice(0, 8),
    [state.history],
  );
  const run = (command) => void act(command).catch(() => {});
  return (
    <div className="page-content dictation-page">
      <div className="page-header">
        <div>
          <div className="eyebrow">MAKE ROOM FOR YOUR WORDS</div>
          <h1 className="page-heading">Dictation</h1>
          <p className="page-subtitle">
            Think out loud. Keep your train of thought.
          </p>
        </div>
        <Badge variant="light" color={state.ready ? "mint" : "gray"}>
          {state.ready ? "Ready to listen" : "Setup needed"}
        </Badge>
      </div>
      <section className="dictation-stage">
        <div className={`mic-emblem ${state.recording ? "listening" : ""}`}>
          <Microphone size={35} weight="duotone" />
        </div>
        <h2>
          {state.recording
            ? "Go ahead. I’m listening."
            : state.busy
              ? "A moment for the finishing touches."
              : "Your next thought starts here."}
        </h2>
        <p>
          {state.recording
            ? formatTime(state.seconds * 1000)
            : state.ready
              ? "Hold your shortcut in any text field, then speak naturally."
              : "Set up your speech model to start dictating on this device."}
        </p>
        {state.ready ? (
          <Group justify="center">
            <Button
              size="md"
              leftSection={
                state.recording ? <Stop size={17} /> : <Microphone size={17} />
              }
              disabled={state.busy || state.note_recording}
              onClick={() =>
                run({
                  type: state.recording ? "dictation_stop" : "dictation_start",
                })
              }
            >
              {state.recording ? "Finish dictation" : "Start dictating"}
            </Button>
            {state.busy && (
              <Button
                variant="subtle"
                color="gray"
                onClick={() => run({ type: "dictation_cancel" })}
              >
                Cancel
              </Button>
            )}
          </Group>
        ) : (
          <Button onClick={settings}>Set up dictation</Button>
        )}
        <div className="dictation-shortcut">
          <Kbd>{binding}</Kbd>
          <span>
            {state.settings.hotkey_mode === "Hold"
              ? "Hold to speak"
              : "Press to start"}
          </span>
        </div>
      </section>
      {(state.text || state.recording || state.dictation_interruption) && (
        <section className="dictation-result">
          <Group justify="space-between">
            <div className="section-heading">
              {state.recording ? "LIVE TRANSCRIPT" : "YOUR WORDS, READY TO USE"}
            </div>
            <Group gap="xs">
              <Button
                variant="subtle"
                size="xs"
                loading={inserting}
                disabled={
                  state.recording || state.busy || state.preview || !state.text
                }
                onClick={async () => {
                  setInserting(true);
                  try {
                    await act({
                      type: "insert_text",
                      text: state.text,
                      line_breaks: Object.fromEntries(
                        (state.settings.input_profiles || [])
                          .filter((p) => p.line_break)
                          .map((p) => [p.app, p.line_break]),
                      ),
                    });
                  } catch {
                    /* The workspace presents the error. */
                  } finally {
                    setInserting(false);
                  }
                }}
              >
                Insert in 3 seconds
              </Button>
              <TranscriptEditor
                key={state.dictation_session?.id || "latest"}
                session={state.dictation_session}
                act={act}
                disabled={
                  state.recording ||
                  state.busy ||
                  state.dictation_session?.text !== state.text
                }
              />
              <Button
                variant="subtle"
                size="xs"
                leftSection={<Copy size={16} />}
                onClick={() => run({ type: "copy", text: state.text })}
              >
                Copy text
              </Button>
            </Group>
          </Group>
          {state.dictation_interruption && (
            <Alert color="yellow" title="Incomplete dictation">
              {state.dictation_interruption} Automatic insertion was skipped.
              Copy the captured text, or choose Insert and focus your
              destination field within three seconds.
            </Alert>
          )}
          <p style={{ whiteSpace: "pre-wrap" }}>
            {state.text ||
              (state.recording ? "Listening..." : "No speech recovered.")}
          </p>
          <span className="muted">{state.status}</span>
        </section>
      )}
      <div className="section-heading recent-heading">
        <span>RECENT DICTATIONS</span>
        <span>{recent.length} saved</span>
      </div>
      <div className="recent-dictations">
        {recent.map((session) => (
          <button key={session.id} onClick={() => void open(session.id)}>
            <span>{formatClock(session.created_ms)}</span>
            <p>{session.preview}</p>
            <ArrowUpRight size={18} />
          </button>
        ))}
        {!recent.length && (
          <p className="empty-message">
            Your finished dictations will be saved here, ready to find and copy.
          </p>
        )}
      </div>
    </div>
  );
}
