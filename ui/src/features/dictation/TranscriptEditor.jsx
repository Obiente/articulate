import { useState } from "react";
import {
  Alert,
  Button,
  Checkbox,
  Group,
  Modal,
  Stack,
  Textarea,
  TextInput,
} from "@mantine/core";
import { PencilSimple } from "@phosphor-icons/react";
import { replaceSpelling, suggestSpelling } from "./transcript-edit";

export function TranscriptEditor({ session, act, disabled }) {
  const [opened, setOpened] = useState(false),
    [draft, setDraft] = useState("");
  const [baseline, setBaseline] = useState("");
  const [remember, setRemember] = useState(false),
    [heard, setHeard] = useState(""),
    [wanted, setWanted] = useState(""),
    [cues, setCues] = useState("");
  const [saving, setSaving] = useState(false),
    [error, setError] = useState("");
  function begin() {
    setDraft(session.text);
    setBaseline(session.text);
    setRemember(false);
    setHeard("");
    setWanted("");
    setCues("");
    setError("");
    setOpened(true);
  }
  function change(value) {
    setDraft(value);
    if (!remember) {
      const suggestion = suggestSpelling(baseline, value);
      setHeard(suggestion?.heard || "");
      setWanted(suggestion?.wanted || "");
    }
  }
  async function save() {
    setSaving(true);
    setError("");
    try {
      await act({
        type: "dictation_edit",
        id: session.id,
        expected_text: baseline,
        text: draft,
        correction: remember
          ? { heard: heard.trim(), wanted: wanted.trim(), cues }
          : null,
      });
      setOpened(false);
    } catch (e) {
      setError(e.message || String(e));
    } finally {
      setSaving(false);
    }
  }
  return (
    <>
      <Button
        size="xs"
        variant="light"
        leftSection={<PencilSimple size={16} />}
        disabled={disabled || !session}
        onClick={begin}
      >
        Edit text
      </Button>
      <Modal
        opened={opened}
        onClose={() => !saving && setOpened(false)}
        title="Edit dictation"
        size="lg"
        centered
        closeOnClickOutside={false}
        closeOnEscape={!saving}
        withCloseButton={!saving}
      >
        <Stack gap="md">
          <Textarea
            label="Dictation text"
            value={draft}
            onChange={(e) => change(e.currentTarget.value)}
            autosize
            minRows={8}
            maxRows={18}
            disabled={saving}
            data-autofocus
          />
          <details>
            <summary>Original transcription</summary>
            <p style={{ whiteSpace: "pre-wrap" }}>
              {session?.original || baseline}
            </p>
          </details>
          <Checkbox
            label="Remember a spelling for future dictations"
            checked={remember}
            onChange={(e) => setRemember(e.currentTarget.checked)}
            disabled={saving}
          />
          {remember && (
            <Stack gap="sm">
              <Group grow align="start">
                <TextInput
                  label="Heard as"
                  value={heard}
                  onChange={(e) => setHeard(e.currentTarget.value)}
                  disabled={saving}
                />
                <TextInput
                  label="Write as"
                  value={wanted}
                  onChange={(e) => setWanted(e.currentTarget.value)}
                  disabled={saving}
                />
              </Group>
              <Group justify="space-between">
                <Button
                  size="xs"
                  variant="default"
                  disabled={saving || !heard.trim() || !wanted.trim()}
                  onClick={() =>
                    setDraft(
                      replaceSpelling(draft, heard.trim(), wanted.trim()),
                    )
                  }
                >
                  Replace all in this dictation
                </Button>
              </Group>
              <TextInput
                label="Context words (optional)"
                description="Use this spelling near these words. Separate words with commas."
                placeholder="For example: osu, project"
                value={cues}
                onChange={(e) => setCues(e.currentTarget.value)}
                disabled={saving}
              />
            </Stack>
          )}
          {error && (
            <Alert color="red" role="alert">
              {error}
            </Alert>
          )}
          <Group justify="flex-end">
            <Button
              variant="default"
              onClick={() => setOpened(false)}
              disabled={saving}
            >
              Cancel
            </Button>
            <Button
              onClick={() => void save()}
              loading={saving}
              disabled={
                disabled ||
                !draft.trim() ||
                (draft === baseline && !remember) ||
                (remember && (!heard.trim() || !wanted.trim()))
              }
            >
              Save changes
            </Button>
          </Group>
        </Stack>
      </Modal>
    </>
  );
}
