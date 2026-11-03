import { Button, Group, Paper, Switch } from "@mantine/core";
import { ArrowClockwise, DownloadSimple } from "@phosphor-icons/react";
import {
  SettingRow,
  SettingsSection,
  SetupStatus,
} from "../../components/SettingRow.jsx";

// Checking for, downloading and installing app updates.
export function UpdateSettings({ controls }) {
  const { state, busy, capturing, modelBusy, pending, action, confirm } =
    controls;
  const updater = state.updater || {};
  return (
    <SettingsSection
      id="settings-updates"
      icon={ArrowClockwise}
      title="App updates"
    >
      <Paper withBorder radius="lg" className="secondary-settings-card">
        <SettingRow
          title="Keep Articulate in the tray"
          description="Closing the window hides it. Choose Quit from the tray to save and exit."
        >
          {controls.toggle("close_to_tray", "Close to tray")}
        </SettingRow>
        <SettingRow
          title="Start with Windows"
          description="Open in the tray when you sign in."
        >
          {controls.toggle("start_with_windows", "Start with Windows")}
        </SettingRow>
        <SettingRow
          title="Automatic update checks"
          description="Check on launch and daily while Articulate is open."
        >
          <Switch
            aria-label="Automatic update checks"
            checked={state.settings.automatic_update_checks === true}
            disabled={busy}
            onChange={(event) =>
              controls.patch(
                "automatic_update_checks",
                event.currentTarget.checked,
              )
            }
          />
        </SettingRow>
        <SettingRow
          title={
            updater.state === "available" || updater.state === "ready"
              ? `Articulate ${updater.version || "update"} is available`
              : "Keep Articulate up to date"
          }
          description={
            updater.state === "latest"
              ? "You're running the latest available release."
              : "Check for new features and fixes. Your notes and downloaded models stay on this computer."
          }
        >
          <Group gap="sm" wrap="wrap" justify="flex-end">
            <Button
              variant="default"
              onClick={() => action("update_check")}
              loading={
                updater.state === "checking" || pending === "update_check"
              }
              disabled={
                busy || ["checking", "downloading"].includes(updater.state)
              }
            >
              Check for updates
            </Button>
            {updater.state === "available" && (
              <Button
                leftSection={<DownloadSimple size={17} />}
                onClick={() => action("update_download")}
                disabled={busy || capturing}
              >
                {updater.size
                  ? `Download · ${Math.ceil(updater.size / 1000000)} MB`
                  : "Download update"}
              </Button>
            )}
            {updater.state === "ready" && (
              <Button
                onClick={() => confirm("update_install")}
                disabled={busy || capturing || state.saving || modelBusy}
              >
                Install update
              </Button>
            )}
          </Group>
        </SettingRow>
        <SetupStatus
          busy={updater.state === "downloading"}
          message={
            updater.state === "downloading"
              ? "Downloading the update…"
              : undefined
          }
          progress={updater.progress}
          error={updater.error}
        />
      </Paper>
    </SettingsSection>
  );
}
