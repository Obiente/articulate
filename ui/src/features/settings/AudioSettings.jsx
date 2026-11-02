import { Badge, Button, Paper, Select } from "@mantine/core";
import { Check, DownloadSimple, Microphone } from "@phosphor-icons/react";
import {
  SettingRow,
  SettingsSection,
  SetupStatus,
} from "../../components/SettingRow.jsx";
import { languageLabel } from "../../lib/format.js";

const devices = (list, selected) =>
  [...new Set([...(list || []), ...(selected ? [selected] : [])])].map(
    (name) => ({ value: name, label: name }),
  );

// Recognition language, live transcription model and audio devices.
export function AudioSettings({ controls }) {
  const { state, settings, models, busy, capturing, modelBusy } = controls;
  const { pending, patch, action } = controls;
  const speechLanguages = [
    ...new Set([
      ...(models.speech_languages ||
        (state.preview ? ["en", "nl", "fr", "de", "es", "ja", "zh"] : [])),
      ...(settings.transcription_language
        ? [settings.transcription_language]
        : []),
    ]),
  ]
    .map((code) => ({ value: code, label: languageLabel(code) }))
    .sort((a, b) => a.label.localeCompare(b.label));
  return (
    <SettingsSection id="settings-audio" icon={Microphone} title="Audio">
      <Paper withBorder radius="lg" className="secondary-settings-card">
        <SettingRow
          title="Transcription language"
          description="Set the recognition language for dictation and spoken notes. Calls have their own language preference and keep automatic detection."
        >
          <Select
            aria-label="Transcription language"
            searchable
            allowDeselect={false}
            data={[{ value: "auto", label: "Auto-detect" }, ...speechLanguages]}
            value={settings.transcription_language || "auto"}
            disabled={busy || capturing || state.loading}
            onChange={(value) => {
              if (value)
                patch("transcription_language", value === "auto" ? "" : value);
            }}
          />
        </SettingRow>
        <SettingRow
          title="Fast live transcription"
          description="Nemotron gives you fast live previews and checks short finished dictations. Qwen prepares the final text. Previews may change as you speak."
        >
          {models.verifier_installed ? (
            <Badge variant="light" leftSection={<Check size={14} />}>
              Installed
            </Badge>
          ) : (
            <Button
              variant="default"
              leftSection={<DownloadSimple size={17} />}
              loading={
                models.verifier_working || pending === "verifier_download"
              }
              disabled={busy || modelBusy || capturing}
              onClick={() => action("verifier_download")}
            >
              Download · 751 MB
            </Button>
          )}
        </SettingRow>
        <SetupStatus
          busy={models.verifier_working}
          message={models.verifier_status}
          progress={models.verifier_progress}
        />
        <SettingRow
          title="Microphone"
          description="Choose the input for dictation and your voice in calls."
        >
          <Select
            aria-label="Microphone"
            placeholder="System default"
            searchable
            clearable
            data={devices(state.microphones, settings.microphone)}
            value={settings.microphone || null}
            onChange={(value) => patch("microphone", value ?? "")}
            disabled={
              busy ||
              state.recording ||
              state.call_recording ||
              state.note_recording
            }
            nothingFoundMessage="No matching microphones"
          />
        </SettingRow>
        <SettingRow
          title="Call audio output"
          description="Choose the device your call plays through."
        >
          <Select
            aria-label="Call audio output"
            placeholder="System default"
            searchable
            clearable
            data={devices(state.outputs, settings.output)}
            value={settings.output || null}
            onChange={(value) => patch("output", value ?? "")}
            disabled={busy || state.call_recording}
            nothingFoundMessage="No matching outputs"
          />
        </SettingRow>
      </Paper>
    </SettingsSection>
  );
}
