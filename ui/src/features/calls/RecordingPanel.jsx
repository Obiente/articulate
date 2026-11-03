import { useState } from "react";
import { Button, Group, Modal, Progress, Text } from "@mantine/core";
import {
  ArrowsClockwise,
  ClockCounterClockwise,
  Play,
  Trash,
  Waveform,
} from "@phosphor-icons/react";
import { formatDate, formatTime } from "../../lib/format.js";
import { formatBytes } from "./speakers.js";

const confirmations = {
  rebuild: {
    title: "Re-transcribe this recording?",
    message:
      "Articulate will rebuild the transcript from the kept audio with the speech model loaded now. Speaker names and corrections start over. The current transcript stays available under Earlier transcripts, and your notes are not changed.",
    action: "Re-transcribe",
  },
  delete: {
    title: "Delete the kept audio?",
    message:
      "The recording is removed from this device. The transcript and notes stay. You will not be able to play turns or re-transcribe this conversation afterwards.",
    action: "Delete audio",
  },
};

// Kept audio for a saved recording: playback, re-transcription and earlier
// transcript versions.
export function RecordingPanel({ session, importState, act, locked, live }) {
  const [confirming, setConfirming] = useState(null),
    [busy, setBusy] = useState(false);
  if (!session || live) return null;
  const audio = session.audio;
  const versions = session.transcript_versions || [];
  const rebuilding = importState?.rebuilding === session.id;
  const otherJob = importState?.state === "working" && !rebuilding;
  if (!audio && !versions.length && !rebuilding) return null;
  const run = async (command) => {
    setBusy(true);
    try {
      await act(command);
      return true;
    } catch {
      return false;
    } finally {
      setBusy(false);
    }
  };
  const confirm = confirmations[confirming] || confirmations.rebuild;
  return (
    <div className="recording-panel">
      <div className="section-heading">
        <span>RECORDING</span>
      </div>
      {audio && (
        <>
          <div className="detail-row">
            <Waveform size={17} />
            <span>
              Audio kept on this device · {formatTime(audio.duration_ms)} ·{" "}
              {formatBytes(audio.bytes)}
            </span>
          </div>
          {audio.incomplete && (
            <Text size="xs" c="yellow" mb="sm">
              Part of the audio could not be kept.
            </Text>
          )}
          <Group gap={6} mb="md">
            <Button
              size="compact-xs"
              variant="light"
              leftSection={<Play size={12} weight="fill" />}
              disabled={busy}
              onClick={() =>
                void run({ type: "audio_play", id: session.id, from_ms: 0 })
              }
            >
              Play
            </Button>
            <Button
              size="compact-xs"
              variant="subtle"
              leftSection={<ArrowsClockwise size={13} />}
              disabled={busy || locked || rebuilding || otherJob}
              onClick={() => setConfirming("rebuild")}
            >
              Re-transcribe
            </Button>
            <Button
              size="compact-xs"
              variant="subtle"
              color="red"
              leftSection={<Trash size={13} />}
              disabled={busy || locked || rebuilding}
              onClick={() => setConfirming("delete")}
            >
              Delete audio
            </Button>
          </Group>
        </>
      )}
      {rebuilding && (
        <div className="recording-progress" role="status">
          <Text size="xs" c="dimmed">
            {importState.stage || "Re-transcribing"}…
          </Text>
          <Progress
            size="sm"
            value={Math.round((importState.progress ?? 0) * 100)}
            aria-label="Re-transcription progress"
          />
          <Button
            size="compact-xs"
            variant="subtle"
            color="gray"
            disabled={busy}
            onClick={() => void run({ type: "audio_import_cancel" })}
          >
            Cancel
          </Button>
        </div>
      )}
      {versions.length > 0 && (
        <div className="transcript-versions">
          <Text size="xs" c="dimmed" mb={6}>
            Earlier transcripts
          </Text>
          {versions.map((version) => (
            <div className="transcript-version" key={version.id}>
              <ClockCounterClockwise size={15} />
              <span title={version.preview}>
                Replaced {formatDate(version.replaced_ms)}
                {version.model ? ` · ${version.model}` : ""}
              </span>
              <Button
                size="compact-xs"
                variant="subtle"
                disabled={busy || locked || rebuilding}
                onClick={() =>
                  void run({
                    type: "transcript_restore",
                    id: session.id,
                    version: version.id,
                    expected: session.transcript_revision,
                  })
                }
              >
                Restore
              </Button>
            </div>
          ))}
        </div>
      )}
      <Modal
        opened={Boolean(confirming)}
        onClose={() => !busy && setConfirming(null)}
        title={confirm.title}
        centered
        size="md"
      >
        <Text c="dimmed">{confirm.message}</Text>
        <Group justify="flex-end" mt="xl">
          <Button
            variant="default"
            disabled={busy}
            onClick={() => setConfirming(null)}
          >
            Cancel
          </Button>
          <Button
            color={confirming === "delete" ? "red" : undefined}
            loading={busy}
            onClick={async () => {
              const done = await run(
                confirming === "delete"
                  ? {
                      type: "audio_delete",
                      id: session.id,
                      confirm: { id: session.id, bytes: audio.bytes },
                    }
                  : { type: "transcript_rebuild", id: session.id },
              );
              if (done) setConfirming(null);
            }}
          >
            {confirm.action}
          </Button>
        </Group>
      </Modal>
    </div>
  );
}
