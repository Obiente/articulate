import { useState } from "react";
import {
  ActionIcon,
  Button,
  Checkbox,
  Group,
  Menu,
  Modal,
  SegmentedControl,
  Stack,
  Text,
  TextInput,
  Tooltip,
} from "@mantine/core";
import { DownloadSimple } from "@phosphor-icons/react";
import { defaultReminder, reminderProblem, reminderTime } from "./exports.js";
import "../library/library.css";

const formats = [
  ["text", "Text", ".txt"],
  ["markdown", "Markdown", ".md"],
  ["srt", "SRT subtitles", ".srt"],
  ["vtt", "WebVTT subtitles", ".vtt"],
];

// Export the open document as text, Markdown or subtitles, or its kept
// action items as a checklist or calendar file. `run(command)` resolves to
// whether the action was accepted.
export function ExportMenu({ session, run, disabled }) {
  const [followUps, setFollowUps] = useState(false);
  const subtitles = session?.subtitles_unavailable;
  const count = session?.follow_ups || 0;
  const exportAs = (format) =>
    run({ type: "session_export", id: session.id, format });
  return (
    <>
      <Menu withinPortal position="bottom-end" width={250}>
        <Menu.Target>
          <Tooltip label="Export">
            <ActionIcon
              size={34}
              variant="subtle"
              color="gray"
              aria-label="Export"
              disabled={disabled || !session}
            >
              <DownloadSimple size={19} />
            </ActionIcon>
          </Tooltip>
        </Menu.Target>
        <Menu.Dropdown>
          <Menu.Label>Export this document</Menu.Label>
          {formats.map(([format, label, extension]) => {
            const blocked = (format === "srt" || format === "vtt") && subtitles;
            return (
              <Menu.Item
                key={format}
                disabled={Boolean(blocked)}
                onClick={() => exportAs(format)}
                rightSection={
                  <Text size="xs" c="dimmed">
                    {extension}
                  </Text>
                }
              >
                {label}
                {blocked && (
                  <Text size="xs" c="dimmed" className="menu-item-reason">
                    {subtitles}
                  </Text>
                )}
              </Menu.Item>
            );
          })}
          <Menu.Divider />
          <Menu.Label>Follow-ups</Menu.Label>
          <Menu.Item disabled={!count} onClick={() => setFollowUps(true)}>
            {count
              ? `Export ${count} ${count === 1 ? "follow-up" : "follow-ups"}…`
              : "Export follow-ups"}
            {!count && (
              <Text size="xs" c="dimmed" className="menu-item-reason">
                Update notes to find action items. Those you keep in your notes
                can be exported.
              </Text>
            )}
          </Menu.Item>
        </Menu.Dropdown>
      </Menu>
      <FollowUpsDialog
        opened={followUps}
        count={count}
        onClose={() => setFollowUps(false)}
        onExport={async (format, reminder) => {
          const accepted = await run({
            type: "followups_export",
            id: session.id,
            format,
            reminder_ms: reminder,
          });
          if (accepted) setFollowUps(false);
        }}
      />
    </>
  );
}

function FollowUpsDialog({ opened, count, onClose, onExport }) {
  const [format, setFormat] = useState("markdown"),
    [remind, setRemind] = useState(false),
    [when, setWhen] = useState(defaultReminder),
    [saving, setSaving] = useState(false);
  const problem = format === "ics" && remind ? reminderProblem(when) : null;
  return (
    <Modal
      opened={opened}
      onClose={() => !saving && onClose()}
      title="Export follow-ups"
      centered
      size="md"
    >
      <Stack gap="md">
        <Text c="dimmed" size="sm">
          {count} {count === 1 ? "action item" : "action items"} from this
          document's notes. Only items still in your notes are included, in your
          words.
        </Text>
        <SegmentedControl
          aria-label="Follow-up format"
          value={format}
          onChange={setFormat}
          data={[
            { value: "markdown", label: "Checklist (.md)" },
            { value: "ics", label: "Calendar tasks (.ics)" },
          ]}
        />
        {format === "ics" && (
          <>
            <Checkbox
              label="Add a reminder"
              description="Calendar apps will remind you about each task at this time."
              checked={remind}
              onChange={(event) => setRemind(event.currentTarget.checked)}
            />
            {remind && (
              <TextInput
                type="datetime-local"
                label="Remind me"
                value={when}
                onChange={(event) => setWhen(event.currentTarget.value)}
                error={problem}
              />
            )}
          </>
        )}
        <Group justify="flex-end">
          <Button variant="default" onClick={onClose} disabled={saving}>
            Cancel
          </Button>
          <Button
            loading={saving}
            disabled={Boolean(problem)}
            onClick={async () => {
              setSaving(true);
              try {
                await onExport(
                  format,
                  format === "ics" && remind ? reminderTime(when) : null,
                );
              } finally {
                setSaving(false);
              }
            }}
          >
            Choose where to save
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
