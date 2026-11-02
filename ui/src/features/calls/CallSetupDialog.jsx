import { useEffect, useState } from "react";
import { Alert, Button, Group, Modal, Select, Switch } from "@mantine/core";
import { Headphones, Users } from "@phosphor-icons/react";
import { languageLabel } from "../../lib/format.js";

// Choose the audio source and language preference for a new conversation.
export function CallSetupDialog({ opened, state, act, onClose, onStarted }) {
  const [source, setSource] = useState("system"),
    [language, setLanguage] = useState("auto"),
    [audioCues, setAudioCues] = useState(true),
    [starting, setStarting] = useState(false);
  // Each opening starts from the saved preferences.
  useEffect(() => {
    if (!opened) return;
    setSource("system");
    setLanguage(state?.settings?.transcription_language || "auto");
    setAudioCues(Boolean(state?.settings?.audio_context));
  }, [opened]);
  const discordReady = Boolean(state?.discord?.audio_ready);
  return (
    <Modal
      opened={opened}
      onClose={() => !starting && onClose()}
      title="Record a conversation"
      centered
      radius="lg"
      size="md"
    >
      {state?.preview && (
        <Alert color="mint" variant="light" mb="md">
          Recording is available in the Articulate desktop app. You can explore
          the audio options here.
        </Alert>
      )}
      <p className="call-setup-intro">
        Choose the audio source for this recording. Your meeting can be in any
        app.
      </p>
      <div
        className="call-source-options"
        role="radiogroup"
        aria-label="Conversation audio source"
      >
        <button
          type="button"
          role="radio"
          aria-checked={source === "system"}
          className={`call-source-option ${source === "system" ? "selected" : ""}`}
          onClick={() => setSource("system")}
        >
          <Headphones size={23} />
          <span>
            <strong>Any meeting app</strong>
            <small>
              Microphone and system output. Speakers are identified from audio
              when available.
            </small>
          </span>
          <span className="call-source-radio" />
        </button>
        <button
          type="button"
          role="radio"
          aria-checked={source === "discord"}
          disabled={!discordReady}
          className={`call-source-option ${source === "discord" ? "selected" : ""}`}
          onClick={() => setSource("discord")}
        >
          <Users size={23} />
          <span>
            <strong>Separate Discord audio</strong>
            <small>
              {discordReady
                ? "Individual participant tracks with Discord names."
                : "Connect the Discord companion to use participant tracks."}
            </small>
          </span>
          <span className="call-source-radio" />
        </button>
      </div>
      <Select
        mt="md"
        label="Language preference for this call"
        description="Helps settle ambiguous short words. Other languages can still be recognized."
        searchable
        allowDeselect={false}
        value={language}
        onChange={(value) => setLanguage(value || "auto")}
        data={[
          { value: "auto", label: "Auto-detect" },
          ...(state?.model_status?.speech_languages || []).map((code) => ({
            value: code,
            label: languageLabel(code),
          })),
        ]}
      />
      {state?.model_status?.context_installed && (
        <Switch
          mt="md"
          label="Sound and tone cues"
          description="Mark sounds and possible vocal emotions in the transcript."
          checked={audioCues}
          onChange={(event) => setAudioCues(event.currentTarget.checked)}
        />
      )}
      <p className="call-setup-device">
        Output: {state?.settings?.output || "System default"} · Microphone:{" "}
        {state?.settings?.microphone || "System default"}
      </p>
      <Group justify="flex-end" mt="lg">
        <Button
          variant="subtle"
          color="gray"
          onClick={onClose}
          disabled={starting}
        >
          Cancel
        </Button>
        <Button
          loading={starting}
          disabled={
            state?.preview ||
            !state?.ready ||
            (source === "discord" && !discordReady)
          }
          onClick={async () => {
            setStarting(true);
            try {
              await act({
                type: "call_start",
                source,
                language_preference: language === "auto" ? null : language,
                audio_context: state?.model_status?.context_installed
                  ? audioCues
                  : false,
              });
              onStarted();
            } catch {
              // The action displays the error.
            } finally {
              setStarting(false);
            }
          }}
        >
          Start recording
        </Button>
      </Group>
    </Modal>
  );
}
