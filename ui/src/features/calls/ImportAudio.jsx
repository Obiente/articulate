import { useEffect, useRef, useState } from "react";
import {
  Alert,
  Button,
  Group,
  Modal,
  Progress,
  SegmentedControl,
  Switch,
  Text,
} from "@mantine/core";
import { FileAudio, UploadSimple } from "@phosphor-icons/react";
import "./calls.css";

// Privacy copy shared by call setup, import and settings.
export const keepAudioDescription =
  "Saved next to the transcript so you can replay a turn or re-transcribe later. It stays on this device, is encrypted with your Windows account when encryption at rest is on, and is deleted with the conversation.";

// Choose how to import an offered file, then follow its progress.
export function ImportAudioDialog({ state, act, openSession }) {
  const job = state?.audio_import;
  const [kind, setKind] = useState("call"),
    [keepAudio, setKeepAudio] = useState(false),
    [starting, setStarting] = useState(false);
  const offered = job?.state === "ready";
  const importing = job?.state === "working" && !job.rebuilding;
  const failed = job?.state === "failed" && !job.rebuilding;
  const finished = job?.state === "done" && !job.rebuilding;
  const opened = useRef(null);
  // Each offered file starts from the saved preference.
  useEffect(() => {
    if (!offered) return;
    setKind("call");
    setKeepAudio(Boolean(state?.settings?.keep_call_audio));
  }, [offered, job?.file_name]);
  // Open a finished import once.
  useEffect(() => {
    if (!finished || !job.session_id || opened.current === job.session_id)
      return;
    opened.current = job.session_id;
    void act({ type: "audio_import_dismiss" })
      .then(() => openSession(job.session_id))
      .catch(() => {});
  }, [finished, job?.session_id]);
  const dismiss = () =>
    void act({ type: "audio_import_dismiss" }).catch(() => {});
  return (
    <Modal
      opened={offered || importing || failed}
      onClose={() => (importing ? undefined : dismiss())}
      withCloseButton={!importing}
      title={importing ? "Importing audio" : "Import audio"}
      centered
      radius="lg"
      size="md"
    >
      <div className="import-file">
        <FileAudio size={26} weight="duotone" />
        <span>{job?.file_name || "Audio file"}</span>
      </div>
      {offered && (
        <>
          {state?.preview && (
            <Alert color="mint" variant="light" mb="md">
              This preview imports a sample conversation. Your files are
              transcribed only in the Articulate desktop app.
            </Alert>
          )}
          <SegmentedControl
            fullWidth
            mt="md"
            value={kind}
            onChange={setKind}
            data={[
              { value: "call", label: "Conversation" },
              { value: "dictation", label: "Dictation" },
            ]}
            aria-label="Import as"
          />
          <Text size="sm" c="dimmed" mt="sm">
            {kind === "call"
              ? job?.speakers_available
                ? "Voices are separated into speakers you can rename afterwards."
                : "Install speaker identification in Settings to separate voices. Without it, the recording is one speaker."
              : "The recording becomes one dictated text."}
          </Text>
          <Switch
            mt="lg"
            label="Keep the audio on this device"
            description={keepAudioDescription}
            checked={keepAudio}
            onChange={(event) => setKeepAudio(event.currentTarget.checked)}
          />
          <Group justify="flex-end" mt="xl">
            <Button variant="subtle" color="gray" onClick={dismiss}>
              Cancel
            </Button>
            <Button
              loading={starting}
              disabled={!state?.ready}
              onClick={async () => {
                setStarting(true);
                try {
                  await act({
                    type: "audio_import_start",
                    kind,
                    keep_audio: keepAudio,
                  });
                } catch {
                  // The shell shows the error.
                } finally {
                  setStarting(false);
                }
              }}
            >
              Import
            </Button>
          </Group>
        </>
      )}
      {importing && (
        <>
          <Text size="sm" c="dimmed" mt="md">
            {job.stage || "Preparing"}… The recording is transcribed on this
            device; you can keep working.
          </Text>
          <Progress
            mt="sm"
            value={Math.round((job.progress ?? 0) * 100)}
            aria-label="Import progress"
            animated
          />
          <Group justify="flex-end" mt="xl">
            <Button
              variant="default"
              onClick={() =>
                void act({ type: "audio_import_cancel" }).catch(() => {})
              }
            >
              Cancel import
            </Button>
          </Group>
        </>
      )}
      {failed && (
        <>
          <Alert color="red" variant="light" mt="md">
            {job.message || "The file could not be imported."}
          </Alert>
          <Group justify="flex-end" mt="xl">
            <Button variant="default" onClick={dismiss}>
              Close
            </Button>
          </Group>
        </>
      )}
    </Modal>
  );
}

// Shown while audio files are dragged over the window.
export function DropOverlay({ dragging }) {
  if (!dragging) return null;
  return (
    <div className="drop-overlay" aria-hidden="true">
      <div>
        <UploadSimple size={34} />
        <strong>Drop to import audio</strong>
        <span>WAV, MP3, M4A, FLAC, Ogg, or the audio of MP4 and MKV</span>
      </div>
    </div>
  );
}

// The "Import audio" button for the conversations and dictation hubs.
export function ImportAudioButton({ act, disabled }) {
  return (
    <Button
      variant="default"
      leftSection={<UploadSimple size={17} />}
      disabled={disabled}
      onClick={() => void act({ type: "audio_import_choose" }).catch(() => {})}
    >
      Import audio
    </Button>
  );
}
