import { useState } from "react";
import {
  Badge,
  Button,
  Group,
  Modal,
  Paper,
  Progress,
  Stack,
  Text,
} from "@mantine/core";
const downloadActions = {
  speech: "model_download",
  verifier: "verifier_download",
  speaker: "speakers_download",
  sound_cues: "context_download",
  polish: "cleanup_download",
  notes: "summary_download",
};
const names = {
  speech: "Speech",
  verifier: "Second speech model",
  speaker: "Speaker recognition",
  sound_cues: "Sound cues",
  polish: "Cleanup",
  notes: "Notes",
};
export function ModelManager({ controls }) {
  const [deletion, setDeletion] = useState(null);
  const state = controls.state,
    models = state.model_status || {};
  const progress = {
    speech: state.downloading,
    verifier: models.verifier_progress,
    speaker: models.speakers_progress,
    sound_cues: models.context_progress,
    polish: models.cleanup_progress,
    notes: state.notes?.progress,
  };
  const working = {
    speech: state.downloading != null,
    verifier: models.verifier_working,
    speaker: models.speakers_working,
    sound_cues: models.context_progress != null,
    polish: models.cleanup_working,
    notes: state.notes?.downloading,
  };
  const cancel = (id) =>
    controls.send(
      id === "polish"
        ? { type: "cleanup_download_cancel" }
        : id === "notes"
          ? { type: "summary_download_cancel" }
          : { type: "model_download_cancel", id },
    );
  return (
    <Stack gap="sm" mb="xl">
      <Text fw={600}>Installed models</Text>
      {(controls.state.managed_models || []).map((model) => (
        <Paper key={model.id} withBorder p="md" radius="lg">
          <Group justify="space-between">
            <Text fw={600}>{names[model.id]}</Text>
            <Group>
              <Badge color={model.installed ? "mint" : "gray"}>
                {model.installed ? "Installed" : "Not installed"}
              </Badge>
              {model.in_use && <Badge>In use</Badge>}
              {!model.installed && (
                <Button
                  disabled={
                    controls.modelBusy ||
                    controls.busy ||
                    controls.state.preview
                  }
                  onClick={() => controls.action(downloadActions[model.id])}
                >
                  Download
                </Button>
              )}
              <Button
                variant="subtle"
                color="red"
                disabled={!model.disk_size || model.in_use}
                onClick={() => setDeletion(model)}
              >
                Delete
              </Button>
            </Group>
          </Group>
          <Text size="sm" c="dimmed">
            {(model.disk_size / 1000000).toFixed(1)} MB
          </Text>
          <Text size="xs" style={{ overflowWrap: "anywhere" }}>
            {model.path}
          </Text>
          {working[model.id] && (
            <>
              <Progress
                mt="sm"
                aria-label={`${names[model.id]} download progress`}
                value={(progress[model.id] || 0) * 100}
                animated
              />
              <Button mt="sm" variant="subtle" onClick={() => cancel(model.id)}>
                Cancel download
              </Button>
            </>
          )}
          {model.in_use && (
            <Text size="sm" c="dimmed">
              Close the task using this model before deleting it. Loaded speech
              models stay in use until you switch models or exit.
            </Text>
          )}
        </Paper>
      ))}
      <Modal
        opened={Boolean(deletion)}
        onClose={() => setDeletion(null)}
        title="Delete downloaded model?"
        centered
      >
        <Text>Your documents are kept. You can download the model again.</Text>
        <Group mt="lg" justify="flex-end">
          <Button variant="default" onClick={() => setDeletion(null)}>
            Keep model
          </Button>
          <Button
            color="red"
            onClick={() => {
              controls.send({ type: "model_delete", id: deletion.id });
              setDeletion(null);
            }}
          >
            Delete model
          </Button>
        </Group>
      </Modal>
    </Stack>
  );
}
