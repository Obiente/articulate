import { useState } from "react";
import {
  Badge,
  Button,
  Group,
  Loader,
  Paper,
  Progress,
  Switch,
  Text,
  TextInput,
} from "@mantine/core";
import {
  ArrowRight,
  BookOpen,
  Check,
  DownloadSimple,
  Sparkle,
  Waveform,
} from "@phosphor-icons/react";
import {
  SettingRow,
  SettingsSection,
  SetupStatus,
} from "../../components/SettingRow.jsx";

// The speech, notes, cleanup, audio context and speaker models.
export function ModelSettings({ controls }) {
  const { state, settings, notes, models, busy, capturing, modelBusy } =
    controls;
  const { pending, patch, action } = controls;
  const [customModel, setCustomModel] = useState("");
  return (
    <>
      <SettingsSection id="settings-models" icon={Sparkle} title="Models">
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title={
              state.ready
                ? "Ready to listen"
                : state.loading
                  ? "Preparing your speech model"
                  : "Set up local transcription"
            }
            description={
              state.ready
                ? "Your speech is transcribed on this computer."
                : state.status ||
                  "Download the speech model once to get started."
            }
          >
            <Group wrap="wrap" gap="sm">
              {state.ready ? (
                <Badge
                  leftSection={<Check size={14} />}
                  size="lg"
                  variant="light"
                >
                  Ready
                </Badge>
              ) : (
                <>
                  <Button
                    variant="default"
                    leftSection={<DownloadSimple size={17} />}
                    onClick={() => action("model_download")}
                    loading={pending === "model_download"}
                    disabled={
                      state.loading ||
                      state.busy ||
                      state.downloading != null ||
                      capturing ||
                      busy
                    }
                  >
                    Download model
                  </Button>
                  <Button
                    leftSection={<ArrowRight size={17} />}
                    onClick={() => action("model_load")}
                    loading={state.loading || pending === "model_load"}
                    disabled={
                      state.busy ||
                      state.downloading != null ||
                      capturing ||
                      busy
                    }
                  >
                    Load model
                  </Button>
                </>
              )}
            </Group>
          </SettingRow>
        </Paper>
      </SettingsSection>
      <Paper withBorder radius="lg" className="secondary-settings-card" mt="sm">
        <SettingRow
          title="Speech processing"
          description={
            models.backend
              ? `Currently using ${models.backend}.`
              : "Use your graphics card when available, or choose CPU processing."
          }
        >
          <Switch
            label="Use CPU"
            checked={Boolean(settings.cpu)}
            disabled={modelBusy || busy}
            onChange={(event) => patch("cpu", event.currentTarget.checked)}
          />
        </SettingRow>
        <details style={{ paddingBottom: 22 }}>
          <summary>Advanced speech model</summary>
          <Text size="sm" c="dimmed" mt="sm">
            {models.model_name
              ? `Current model: ${models.model_name}`
              : "Use your own compatible GGUF speech model."}
          </Text>
          <TextInput
            mt="md"
            label="Custom model file"
            placeholder="Full path to a compatible .gguf model"
            value={customModel}
            onChange={(event) => setCustomModel(event.currentTarget.value)}
            disabled={modelBusy || busy}
          />
          <Group justify="flex-end" mt="sm">
            <Button
              variant="default"
              disabled={!customModel.trim() || modelBusy || busy}
              loading={pending === "model_path"}
              onClick={() => patch("model_path", customModel.trim())}
            >
              Use this model
            </Button>
          </Group>
        </details>
      </Paper>
      {state.downloading != null && (
        <Paper withBorder radius="lg" className="secondary-download">
          <Group justify="space-between" mb="sm">
            <Text fw={500}>Downloading speech model</Text>
            <Text c="dimmed" size="sm">
              {Math.round(state.downloading * 100)}%
            </Text>
          </Group>
          <Progress
            value={state.downloading * 100}
            size="sm"
            radius="xl"
            aria-label="Speech model download progress"
          />
        </Paper>
      )}
      <SettingsSection icon={BookOpen} title="Notes model">
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title={
              notes.ready
                ? "Ready for your conversations"
                : notes.downloading
                  ? "Downloading your notes model"
                  : "Turn conversations into useful notes"
            }
            description="Generate notes and descriptive call titles on this computer. Download once, then use it offline."
          >
            <Group gap="sm" wrap="wrap">
              {notes.ready ? (
                <Badge
                  leftSection={<Check size={14} />}
                  size="lg"
                  variant="light"
                >
                  Ready
                </Badge>
              ) : (
                <Button
                  leftSection={<DownloadSimple size={17} />}
                  onClick={() => action("summary_download")}
                  loading={notes.downloading || pending === "summary_download"}
                  disabled={busy || modelBusy}
                >
                  Download notes model
                </Button>
              )}
              {notes.downloading && (
                <Button
                  variant="subtle"
                  onClick={() => action("summary_download_cancel")}
                  disabled={busy}
                >
                  Cancel download
                </Button>
              )}
            </Group>
          </SettingRow>
          {notes.ready && (
            <SettingRow
              title="Faster local notes"
              description={
                notes.acceleration_installed
                  ? "Acceleration tools are installed. A compatible graphics card is used when available; otherwise notes use the CPU."
                  : "Use a compatible graphics card to generate notes faster. Download the optional acceleration tools once (32 MB)."
              }
            >
              {notes.acceleration_installed ? (
                <Badge variant="light">Tools installed</Badge>
              ) : (
                <Button
                  variant="default"
                  onClick={() => action("summary_download")}
                  loading={notes.downloading || pending === "summary_download"}
                  disabled={busy || notes.working || capturing || modelBusy}
                >
                  Download acceleration
                </Button>
              )}
            </SettingRow>
          )}
          {(notes.download_status || notes.working) && (
            <div className="secondary-model-status" role="status">
              <Group align="flex-start" gap="sm">
                {(notes.downloading || notes.working) && <Loader size="xs" />}
                <Text c="dimmed" size="sm">
                  {notes.working
                    ? notes.status || "Updating your notes…"
                    : notes.download_status}
                </Text>
              </Group>
              {notes.working && notes.progress != null && (
                <Progress
                  value={Math.max(0, Math.min(100, notes.progress * 100))}
                  size="sm"
                  radius="xl"
                  mt="md"
                  aria-label="Notes progress"
                />
              )}
            </div>
          )}
        </Paper>
      </SettingsSection>
      <SettingsSection icon={Sparkle} title="Text cleanup">
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Local writing editor"
            description="Download the editor for advanced text cleanup. Automatic filler and repetition removal works without this download."
          >
            {models.cleanup_installed ? (
              <Badge variant="light" leftSection={<Check size={14} />}>
                Installed
              </Badge>
            ) : (
              <Button
                leftSection={<DownloadSimple size={17} />}
                loading={
                  models.cleanup_working || pending === "cleanup_download"
                }
                disabled={busy || modelBusy}
                onClick={() => action("cleanup_download")}
              >
                Download cleanup model
              </Button>
            )}
          </SettingRow>
          <SetupStatus
            busy={models.cleanup_working}
            message={models.cleanup_status}
            progress={models.cleanup_progress}
          />
          {models.cleanup_working && (
            <Button
              variant="subtle"
              mb="md"
              onClick={() => action("cleanup_download_cancel")}
              disabled={busy}
            >
              Cancel download
            </Button>
          )}
        </Paper>
      </SettingsSection>
      <SettingsSection icon={Waveform} title="Audio context">
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="SenseVoice audio cues"
            description="Show laughter, crying and possible vocal tone beside the speaker. Runs locally in the background."
          >
            <Button
              variant="default"
              leftSection={<DownloadSimple size={17} />}
              loading={
                models.context_progress != null ||
                pending === "context_download"
              }
              disabled={
                busy ||
                modelBusy ||
                state.preview ||
                models.context_supported === false
              }
              onClick={() => action("context_download")}
            >
              {models.context_installed
                ? "Reinstall audio context"
                : "Download · 259 MB"}
            </Button>
          </SettingRow>
          <SetupStatus
            busy={models.context_progress != null}
            message={models.context_status}
            progress={models.context_progress}
          />
          <SettingRow
            title="Include sound and tone cues"
            description="New recordings are checked for sound events and non-neutral vocal tone. Some passages have no cue; spoken words stay unchanged."
          >
            <Switch
              aria-label="Include sound and tone cues"
              size="md"
              checked={Boolean(settings.audio_context)}
              disabled={busy || modelBusy || !models.context_installed}
              onChange={(event) =>
                patch("audio_context", event.currentTarget.checked)
              }
            />
          </SettingRow>
        </Paper>
      </SettingsSection>
      <SettingsSection icon={Waveform} title="Speaker recognition">
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Recognize speakers in mixed audio"
            description="Useful for calls without separate participant audio. The Vencord companion already identifies each participant directly."
          >
            {models.speakers_installed ? (
              <Badge variant="light" leftSection={<Check size={14} />}>
                Installed
              </Badge>
            ) : (
              <Button
                variant="default"
                leftSection={<DownloadSimple size={17} />}
                loading={
                  models.speakers_working || pending === "speakers_download"
                }
                disabled={busy || modelBusy}
                onClick={() => action("speakers_download")}
              >
                Download speaker model
              </Button>
            )}
          </SettingRow>
          <SetupStatus
            busy={models.speakers_working}
            message={models.speakers_status}
            progress={models.speakers_progress}
          />
        </Paper>
      </SettingsSection>
    </>
  );
}
