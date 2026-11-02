import { useState } from "react";
import { Alert, Button, Group, Modal, Text } from "@mantine/core";
import { ShieldCheck } from "@phosphor-icons/react";
import { Heading } from "../../components/Page.jsx";
import { DiscordSettings } from "../discord/DiscordSettings.jsx";
import { MeetingSettings } from "../meetings/MeetingSettings.jsx";
import { ModelSettings } from "../models/ModelSettings.jsx";
import { UpdateSettings } from "../updates/UpdateSettings.jsx";
import { AudioSettings } from "./AudioSettings.jsx";
import { DictationSettings } from "./DictationSettings.jsx";
import { ShortcutDialog } from "./ShortcutDialog.jsx";
import { settingsControls } from "./controls.jsx";

// Jump targets, also used by update notices (`settings-<id>`).
const sections = [
  ["dictation", "Dictation"],
  ["audio", "Audio"],
  ["discord", "Discord"],
  ["meetings", "Meetings"],
  ["models", "Models"],
  ["updates", "Updates"],
];

const confirmations = {
  discord_relaunch: {
    title: "Relaunch Discord?",
    message:
      "Discord will restart with local debugging enabled. Any current voice call will disconnect. Restart Discord normally later to turn debugging off.",
    action: "Relaunch Discord",
  },
  update_install: {
    title: "Install the update?",
    message:
      "Your notes will be saved before the updater starts. Articulate will close to finish installing the update.",
    action: "Install update",
  },
};

export function SettingsPage({ state, act }) {
  const [pending, setPending] = useState("");
  const [shortcut, setShortcut] = useState(null);
  const [confirmation, setConfirmation] = useState(null);
  const controls = settingsControls({
    state,
    act,
    pending,
    setPending,
    editShortcut: (target, current) =>
      setShortcut({ target, value: current, open: true }),
    confirm: setConfirmation,
  });
  const closeShortcut = () =>
    setShortcut((current) => current && { ...current, open: false });
  const confirming = confirmation
    ? confirmations[confirmation]
    : confirmations.update_install;
  return (
    <div className="secondary-page secondary-settings">
      <Heading title="Settings">
        Your shortcuts, models, and connected apps, in one place.
      </Heading>
      {state.preview && (
        <Alert title="Browser preview" color="mint" mb="lg">
          Use the Articulate desktop app to install models, record audio, and
          connect Discord. The documents here are examples.
        </Alert>
      )}
      <Group gap="xs" mb="xl" aria-label="Settings sections">
        {sections.map(([id, label]) => (
          <Button
            key={id}
            variant="light"
            size="sm"
            onClick={() =>
              document
                .getElementById(`settings-${id}`)
                ?.scrollIntoView({ block: "start", behavior: "smooth" })
            }
          >
            {label}
          </Button>
        ))}
      </Group>
      <DictationSettings controls={controls} />
      <AudioSettings controls={controls} />
      <DiscordSettings controls={controls} />
      <MeetingSettings controls={controls} />
      <ModelSettings controls={controls} />
      <UpdateSettings controls={controls} />
      <Text c="dimmed" size="sm" className="secondary-privacy">
        <ShieldCheck size={17} />
        Your transcripts, vocabulary, and preferences stay on this device.
      </Text>
      <Modal
        opened={Boolean(confirmation)}
        onClose={() => !pending && setConfirmation(null)}
        title={confirming.title}
        centered
        size="md"
      >
        <Text c="dimmed">{confirming.message}</Text>
        <Group justify="flex-end" mt="xl">
          <Button
            variant="default"
            disabled={Boolean(pending)}
            onClick={() => setConfirmation(null)}
          >
            Cancel
          </Button>
          <Button
            loading={Boolean(pending)}
            onClick={async () => {
              if (await controls.action(confirmation)) setConfirmation(null);
            }}
          >
            {confirming.action}
          </Button>
        </Group>
      </Modal>
      <ShortcutDialog
        shortcut={shortcut}
        pending={pending}
        onChange={(value) => setShortcut({ ...shortcut, value })}
        onClose={closeShortcut}
        onSave={async () => {
          if (await controls.patch(shortcut.target, shortcut.value))
            closeShortcut();
        }}
      />
    </div>
  );
}
