import { useEffect, useState } from "react";
import {
  Badge,
  Button,
  Group,
  Paper,
  Select,
  Stack,
  Text,
} from "@mantine/core";
import { Waveform } from "@phosphor-icons/react";
import {
  SettingRow,
  SettingsSection,
  SetupStatus,
} from "../../components/SettingRow.jsx";

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
  const [discordMode, setDiscordMode] = useState(
    settings.discord_companion ? "companion" : "standard",
  );
  useEffect(() => {
    setDiscordMode(settings.discord_companion ? "companion" : "standard");
  }, [settings.discord_companion]);
  return (
    <SettingsSection
      id="settings-discord"
      icon={Waveform}
      title="Discord calls"
    >
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
        {usingCompanion && (
          <>
            <SettingRow
              title="Companion pairing"
              description={
                discord.pairing_state === "waiting"
                  ? "Use the latest companion, then restart Discord. Pairing happens automatically."
                  : "The companion pairs automatically with Articulate on this computer."
              }
            >
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
              companion.plugin_status !== "current") && (
              <Button
                onClick={() => action("companion_install")}
                loading={companion.busy && pending !== "companion_detect"}
                disabled={companion.busy || capturing || busy}
              >
                {companionNeedsUpdate
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
    </SettingsSection>
  );
}
