import {
  Button,
  Group,
  Paper,
  SegmentedControl,
  Switch,
  Text,
} from "@mantine/core";
import { BookOpen, Keyboard } from "@phosphor-icons/react";
import { SettingRow, SettingsSection } from "../../components/SettingRow.jsx";
import { shortcutKeys } from "../../lib/format.js";

import { InputProfiles } from "../dictation/InputProfiles.jsx";

const defaultHotkey = { ctrl: true, alt: true, shift: false, win: false };

// Shortcuts and dictation behavior, plus the spoken-note shortcut.
export function DictationSettings({ controls }) {
  const { settings, busy, capturing, patch, toggle, editShortcut } = controls;
  return (
    <>
      <SettingsSection
        id="settings-dictation"
        icon={Keyboard}
        title="Dictation"
      >
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Keyboard shortcut"
            description="Use this shortcut in any app to start dictating."
          >
            <Group gap="md">
              <Group gap={5}>
                {shortcutKeys(settings.hotkey).map((key) => (
                  <kbd key={key}>{key}</kbd>
                ))}
              </Group>
              <Button
                variant="default"
                size="sm"
                disabled={busy || capturing}
                onClick={() =>
                  editShortcut("hotkey", {
                    ...(settings.hotkey || { ...defaultHotkey, key: "Space" }),
                  })
                }
              >
                Change
              </Button>
            </Group>
          </SettingRow>
          {settings.shortcut_error && (
            <Text c="red" role="alert" px="lg">
              {settings.shortcut_error}
            </Text>
          )}
          <SettingRow
            title="Shortcut behavior"
            description={
              settings.hotkey_mode === "Toggle"
                ? "Press once to start, then again to finish."
                : "Keep the shortcut held while you speak. Release to finish."
            }
          >
            <SegmentedControl
              aria-label="Shortcut behavior"
              data={[
                { value: "Hold", label: "Hold to talk" },
                { value: "Toggle", label: "Toggle" },
              ]}
              value={settings.hotkey_mode || "Hold"}
              onChange={(value) => patch("hotkey_mode", value)}
              disabled={busy}
            />
          </SettingRow>
          <SettingRow
            title="Sound feedback"
            description="Hear a cue when dictation, calls, or spoken notes start and stop."
          >
            {toggle("audio_feedback", "Sound feedback")}
          </SettingRow>
          <SettingRow
            title="Clean up speech"
            description="Remove fillers, repetitions, and clear spoken corrections automatically."
          >
            {toggle("clean_speech", "Clean up speech")}
          </SettingRow>
          <SettingRow
            title="Insert into the active field"
            description="Put your finished words where your cursor is."
          >
            {toggle("insert", "Insert into the active field")}
          </SettingRow>
          <SettingRow
            title="Show words as you speak"
            description="Update the active text field during dictation."
          >
            <Switch
              aria-label="Show words as you speak"
              size="md"
              checked={Boolean(settings.live_insert)}
              disabled={busy || !settings.insert}
              onChange={(event) =>
                patch("live_insert", event.currentTarget.checked)
              }
            />
          </SettingRow>
          <SettingRow
            title="Spoken editing commands"
            description="Say a command on its own: new line, new paragraph, scratch that, delete that, or undo that. With commands on, live typing waits for finalized speech."
          >
            {toggle("spoken_editing_commands", "Spoken editing commands")}
          </SettingRow>
          <SettingRow
            title="Remember corrections"
            description="Learn short corrections you make after dictating."
          >
            {toggle("learn_corrections", "Remember corrections")}
          </SettingRow>
        </Paper>
      </SettingsSection>
      <InputProfiles controls={controls} />
      <SettingsSection icon={BookOpen} title="Spoken notes">
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Quick note shortcut"
            description="Press once to speak your thoughts into a note. Press again to finish. Your words stay in Notetaker."
          >
            <Group gap="md">
              <Group gap={5}>
                {settings.quick_note_hotkey ? (
                  shortcutKeys(settings.quick_note_hotkey).map((key) => (
                    <kbd key={key}>{key}</kbd>
                  ))
                ) : (
                  <Text c="dimmed" size="sm">
                    Not configured
                  </Text>
                )}
              </Group>
              <Button
                variant="default"
                size="sm"
                disabled={busy || capturing}
                onClick={() =>
                  editShortcut("quick_note_hotkey", {
                    ...(settings.quick_note_hotkey || {
                      ...defaultHotkey,
                      key: "N",
                    }),
                  })
                }
              >
                Change
              </Button>
            </Group>
          </SettingRow>
          {settings.quick_note_hotkey_pending && (
            <Text size="sm" c="dimmed" px="lg" pb="md" role="status">
              Checking your quick note shortcut…
            </Text>
          )}
          {settings.quick_note_shortcut_error && (
            <Text size="sm" c="red" px="lg" pb="md" role="alert">
              {settings.quick_note_shortcut_error}
            </Text>
          )}
        </Paper>
      </SettingsSection>
    </>
  );
}
