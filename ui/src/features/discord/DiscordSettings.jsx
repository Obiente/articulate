import { useEffect, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Group,
  Modal,
  Paper,
  Select,
  Stack,
  Text,
} from "@mantine/core";
import { ShieldWarning, Waveform } from "@phosphor-icons/react";
import {
  SettingRow,
  SettingsSection,
  SetupStatus,
} from "../../components/SettingRow.jsx";
import { DiscordParticipants } from "./DiscordParticipants.jsx";

const companionRuntimes = {
  current: {
    label: "Latest companion running",
    color: "mint",
    description: "Discord is connected with the latest companion.",
  },
  restart_required: {
    label: "Restart Discord",
    color: "yellow",
    description:
      "The latest companion is installed. Quit Discord completely, including its tray icon, then reopen it to load the update. This will disconnect any active call.",
  },
  update_required: {
    label: "Update needed",
    color: "yellow",
    description:
      "Update the companion to install the current plugin and separate audio support.",
  },
  offline: {
    label: "Installed, not connected",
    color: "gray",
    description:
      "The latest companion is installed. Open Discord and connect to verify the version it is running.",
  },
  unverified: {
    label: "Not verified",
    color: "gray",
    description:
      "Check the installation and connect to Discord to verify the companion.",
  },
};

const editionLabels = {
  stable: "Discord",
  ptb: "Discord PTB",
  canary: "Discord Canary",
};

// Actions that interrupt a call or invalidate the companion's pairing are
// confirmed here, with the consequence spelled out.
const confirmations = {
  discord_relaunch_normally: {
    title: "Restart Discord without the debugger?",
    message:
      "Discord will close and reopen normally, which disconnects any current voice call. The local debugger port closes with it, so other programs can no longer read or control that Discord window.",
    action: "Restart Discord",
  },
  discord_rotate_key: {
    title: "Rotate the pairing key?",
    message:
      "Articulate generates a new pairing key. Every copy of the old key stops working at once. The current companion picks up the new key from this computer within a few seconds and pairs again by itself; older companions must be updated first.",
    action: "Rotate key",
  },
  discord_forget_pairing: {
    title: "Forget the companion pairing?",
    message:
      "The pairing key is removed and Articulate stops listening for the companion. Nothing can connect with the old key again. Choose Connect later to pair again with a fresh key; the companion must be running in Discord to pick it up.",
    action: "Forget pairing",
  },
};

// Discord connection, companion setup and automatic call capture.
export function DiscordSettings({ controls }) {
  const { state, settings, busy, capturing, pending, action, toggle } =
    controls;
  const discord = state.discord || {};
  const discordListening = Boolean(discord.listener_started);
  const usingCompanion = Boolean(
    discord.companion_selected ?? settings.discord_companion,
  );
  const discordStatus = {
    ready: discord.connected ? "Connected to Discord" : "Waiting for Discord",
    connecting: usingCompanion
      ? "Waiting for the companion"
      : "Connecting to Discord…",
    unavailable: "Discord is not available yet",
    disconnected: "Not connected",
  }[discord.status];
  const companion = discord.companion || {};
  const companionRuntime =
    companionRuntimes[companion.runtime_status || "unverified"];
  const companionNeedsUpdate =
    companion.runtime_status === "update_required" ||
    companion.plugin_status === "update_available";
  const protocol = discord.companion_protocol;
  const installedFlavors = discord.installed_flavors || [];
  const [discordMode, setDiscordMode] = useState(
    settings.discord_companion ? "companion" : "standard",
  );
  const [confirmation, setConfirmation] = useState(null);
  useEffect(() => {
    setDiscordMode(settings.discord_companion ? "companion" : "standard");
  }, [settings.discord_companion]);
  const confirming = confirmation
    ? confirmations[confirmation]
    : confirmations.discord_rotate_key;
  const openUpdates = () =>
    document
      .getElementById("settings-updates")
      ?.scrollIntoView({ block: "start", behavior: "smooth" });
  return (
    <SettingsSection
      id="settings-discord"
      icon={Waveform}
      title="Discord calls"
    >
      {discord.debugger_open && (
        <Alert
          color="yellow"
          variant="light"
          icon={<ShieldWarning size={20} />}
          title="Discord is running in debugger mode"
          mb="md"
          role="alert"
        >
          <Text size="sm">
            {discord.debugger_notice ||
              "Any program on this computer can read or control that Discord window until you restart Discord without the debugger."}
            {discord.debugger_port
              ? ` The debugger listens on loopback port ${discord.debugger_port}.`
              : ""}
          </Text>
          <Button
            mt="sm"
            size="sm"
            color="yellow"
            variant="light"
            loading={
              discord.relaunching || pending === "discord_relaunch_normally"
            }
            disabled={capturing || busy || discord.relaunching}
            onClick={() => setConfirmation("discord_relaunch_normally")}
          >
            Relaunch Discord normally
          </Button>
        </Alert>
      )}
      {(protocol === "outdated" || protocol === "newer") && (
        <Alert
          color={protocol === "outdated" ? "yellow" : "red"}
          variant="light"
          title={
            protocol === "outdated"
              ? "Update the companion"
              : "Update Articulate"
          }
          mb="md"
          role="alert"
        >
          <Text size="sm">
            {discord.companion_protocol_message ||
              (protocol === "outdated"
                ? "The Discord companion speaks an older protocol. Update it, then restart Discord."
                : "The Discord companion is newer than this Articulate version.")}
          </Text>
          {protocol === "outdated" ? (
            <Button
              mt="sm"
              size="sm"
              onClick={() => action("companion_install")}
              loading={companion.busy && pending !== "companion_detect"}
              disabled={companion.busy || capturing || busy}
            >
              Update companion
            </Button>
          ) : (
            <Button mt="sm" size="sm" variant="light" onClick={openUpdates}>
              Check for app updates
            </Button>
          )}
        </Alert>
      )}
      <Paper withBorder radius="lg" className="secondary-settings-card">
        <SettingRow
          title="Discord connection"
          description={
            discordStatus ||
            "Choose how Articulate connects to your Discord voice channel."
          }
        >
          <Stack gap="sm" align="flex-end">
            <Select
              label="Connection method"
              allowDeselect={false}
              aria-label="Discord connection method"
              value={discordMode}
              onChange={(value) => value && setDiscordMode(value)}
              disabled={discordListening || companion.busy || busy}
              data={[
                { value: "companion", label: "Vencord companion" },
                { value: "standard", label: "Standard Discord" },
              ]}
            />
            <Group gap="sm">
              <Badge
                variant="light"
                color={discord.connected ? "mint" : "gray"}
              >
                {discordStatus || "Not connected"}
              </Badge>
              <Button
                variant="default"
                loading={
                  pending === "discord_connect" ||
                  pending === "discord_disconnect"
                }
                disabled={busy || discord.relaunching || capturing}
                onClick={() =>
                  discordListening
                    ? action("discord_disconnect")
                    : action("discord_connect", {
                        companion: discordMode === "companion",
                      })
                }
              >
                {discordListening ? "Disconnect" : "Connect"}
              </Button>
            </Group>
          </Stack>
        </SettingRow>
        {discord.error && <SetupStatus error={discord.error} />}
        {discord.connected && (
          <>
            <SettingRow
              title="Voice channel"
              description={
                discord.in_voice
                  ? "Who is in the call right now, and who is speaking."
                  : "Participants appear here once you join a voice channel."
              }
            >
              <Badge variant="light" color={discord.in_voice ? "mint" : "gray"}>
                {discord.in_voice ? "In a voice channel" : "Not in a channel"}
              </Badge>
            </SettingRow>
            <DiscordParticipants
              participants={discord.participants}
              inVoice={discord.in_voice}
            />
          </>
        )}
        <SettingRow
          title="Discord edition"
          description={
            installedFlavors.length > 1
              ? "Which installed Discord to restart and read activity from. Stable is used unless you choose another edition."
              : installedFlavors.length === 1
                ? `${editionLabels[installedFlavors[0]] || installedFlavors[0]} is the only installed edition.`
                : "No Discord installation was found yet. The list updates after Discord is installed or connected."
          }
        >
          <Select
            aria-label="Discord edition"
            allowDeselect={false}
            value={settings.discord_flavor || ""}
            disabled={
              busy ||
              discord.relaunching ||
              discord.debugger_open ||
              capturing ||
              installedFlavors.length === 0
            }
            onChange={(value) =>
              value !== null && controls.patch("discord_flavor", value)
            }
            data={[
              {
                value: "",
                label:
                  installedFlavors.length > 1
                    ? "Automatic (prefer Discord)"
                    : "Automatic",
              },
              ...installedFlavors.map((id) => ({
                value: id,
                label: editionLabels[id] || id,
              })),
              ...(settings.discord_flavor &&
              !installedFlavors.includes(settings.discord_flavor)
                ? [
                    {
                      value: settings.discord_flavor,
                      label: `${editionLabels[settings.discord_flavor] || settings.discord_flavor} (not found)`,
                    },
                  ]
                : []),
            ]}
          />
        </SettingRow>
        {usingCompanion && (
          <>
            <SettingRow
              title="Companion pairing"
              description={
                discord.pairing_state === "waiting"
                  ? "Use the latest companion, then restart Discord. Pairing happens automatically."
                  : discord.pairing_key_present
                    ? "The companion pairs automatically with Articulate on this computer. Rotate the key to lock out every copy of the old one; forget the pairing to stop listening entirely."
                    : "No pairing key is stored. Connecting creates a fresh key that the companion picks up automatically."
              }
            >
              <Stack gap="xs" align="flex-end">
                <Badge
                  variant="light"
                  color={discord.pairing_state === "paired" ? "mint" : "gray"}
                >
                  {discord.pairing_state === "paired"
                    ? "Paired"
                    : discord.pairing_state === "waiting"
                      ? "Waiting for companion"
                      : "Automatic pairing"}
                </Badge>
                <Group gap="xs" justify="flex-end" wrap="wrap">
                  <Button
                    variant="default"
                    size="sm"
                    loading={pending === "discord_rotate_key"}
                    disabled={busy || capturing}
                    onClick={() => setConfirmation("discord_rotate_key")}
                  >
                    Rotate pairing key
                  </Button>
                  <Button
                    variant="subtle"
                    color="red"
                    size="sm"
                    loading={pending === "discord_forget_pairing"}
                    disabled={busy || capturing || !discord.pairing_key_present}
                    onClick={() => setConfirmation("discord_forget_pairing")}
                  >
                    Forget pairing
                  </Button>
                </Group>
              </Stack>
            </SettingRow>
            <SettingRow
              title="Participant audio"
              description={
                discord.audio_message ||
                (discord.audio_ready
                  ? "Each participant's audio can be transcribed separately."
                  : discord.connected
                    ? "Speaker names are connected. Waiting for the companion's separate audio connection."
                    : "The separate audio connection becomes available after the companion connects.")
              }
            >
              <Stack gap="xs" align="flex-end">
                <Badge
                  variant="light"
                  color={discord.audio_ready ? "mint" : "gray"}
                >
                  {discord.audio_ready
                    ? "Separate audio ready"
                    : "Audio not ready"}
                </Badge>
                {discord.connected && (
                  <Text c="dimmed" size="sm">
                    {discord.in_voice
                      ? "Voice channel joined"
                      : "Waiting for a voice channel"}
                  </Text>
                )}
              </Stack>
            </SettingRow>
          </>
        )}
        <SettingRow
          title="Vencord companion"
          description="Keep each participant's audio separate, with their Discord name attached to the transcript."
        >
          <Group gap="sm" wrap="wrap" justify="flex-end">
            <Button
              variant="default"
              onClick={() => action("companion_detect")}
              loading={pending === "companion_detect"}
              disabled={companion.busy || busy}
            >
              Check installation
            </Button>
            {(companionNeedsUpdate ||
              protocol === "outdated" ||
              companion.plugin_status !== "current") && (
              <Button
                onClick={() => action("companion_install")}
                loading={companion.busy && pending !== "companion_detect"}
                disabled={companion.busy || capturing || busy}
              >
                {companionNeedsUpdate || protocol === "outdated"
                  ? "Update companion"
                  : "Install companion"}
              </Button>
            )}
            {(companion.installed ||
              companion.source_found ||
              companion.plugin_status === "current") && (
              <Button
                variant="subtle"
                onClick={() => action("companion_install")}
                disabled={companion.busy || capturing || busy}
              >
                Repair companion
              </Button>
            )}
            <Badge variant="light" color={companionRuntime?.color || "gray"}>
              {companionRuntime?.label || "Not verified"}
            </Badge>
            {companion.installer_ready && (
              <Button
                variant="default"
                onClick={() => action("companion_open_installer")}
                disabled={companion.busy || capturing || busy}
              >
                Open Discord installer
              </Button>
            )}
          </Group>
        </SettingRow>
        <SetupStatus message={companionRuntime?.description} />
        <SetupStatus
          busy={companion.busy}
          message={companion.busy ? companion.status : undefined}
          error={companion.error}
        />
        <SettingRow
          title="Keep the companion up to date"
          description="Update the installed companion when a newer version is available."
        >
          {toggle(
            "vencord_auto_update",
            "Update the Vencord companion automatically",
          )}
        </SettingRow>
        {discordMode === "standard" && !settings.discord_companion && (
          <SettingRow
            title="Standard Discord setup"
            description="Restart Discord with local debugging enabled so Articulate can read speaker activity. This briefly disconnects your call."
          >
            <Button
              variant="default"
              loading={discord.relaunching}
              disabled={capturing || busy || discord.relaunching}
              onClick={() => controls.confirm("discord_relaunch")}
            >
              Relaunch Discord
            </Button>
          </SettingRow>
        )}
        <SettingRow
          title="Participant pictures"
          description="Show Discord profile pictures next to participants. Pictures are fetched from Discord over HTTPS and kept in a small local cache; turn this off to show initials only."
        >
          {toggle("discord_avatars", "Show Discord participant pictures")}
        </SettingRow>
        <SettingRow
          title="Connect automatically"
          description="Connect to Discord when Articulate is open."
        >
          {toggle("discord_auto_connect", "Connect to Discord automatically")}
        </SettingRow>
        <SettingRow
          title="Transcribe calls automatically"
          description="Start when you join a voice channel. Finish and save when you leave."
        >
          {toggle(
            "discord_auto_transcribe",
            "Transcribe Discord calls automatically",
          )}
        </SettingRow>
        <SetupStatus message={discord.automatic_status} />
        {discord.automatic_can_retry && (
          <Group justify="flex-end" pb="lg">
            <Button
              variant="light"
              loading={pending === "discord_retry"}
              disabled={capturing || busy || discord.relaunching}
              onClick={() => action("discord_retry")}
            >
              Retry automatic capture
            </Button>
          </Group>
        )}
      </Paper>
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
            color={
              confirmation === "discord_forget_pairing" ? "red" : undefined
            }
            loading={Boolean(pending)}
            onClick={async () => {
              if (await action(confirmation)) setConfirmation(null);
            }}
          >
            {confirming.action}
          </Button>
        </Group>
      </Modal>
    </SettingsSection>
  );
}
