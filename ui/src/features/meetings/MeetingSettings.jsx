// Meeting detection for Teams, Zoom and Google Meet.
import {
  Alert,
  Badge,
  Button,
  Group,
  Paper,
  SegmentedControl,
  Text,
} from "@mantine/core";
import { VideoCamera, Warning } from "@phosphor-icons/react";
import {
  SettingRow,
  SettingsSection,
  SetupStatus,
} from "../../components/SettingRow.jsx";
import { formatEventTime, meetingApps } from "./meeting-notices.js";
import "../discord/participants.css";

const appDescriptions = {
  teams: "Notice Teams meetings while the Teams app is using your microphone.",
  zoom: "Notice Zoom meetings while the Zoom app is using your microphone.",
  meet: "Best effort: a browser using the microphone counts as a Meet call, so other web calls are noticed too.",
};

export function MeetingSettings({ controls }) {
  const { state, settings, busy, capturing, pending, patch, toggle, action } =
    controls;
  const meetings = state.meetings || {};
  const active = meetings.active || [];
  const events = (meetings.events || []).slice(-5).reverse();
  const prompt = meetings.prompt;
  return (
    <SettingsSection
      id="settings-meetings"
      icon={VideoCamera}
      title="Meeting detection"
    >
      {meetings.probe_error && (
        <Alert
          color="red"
          variant="light"
          icon={<Warning size={20} />}
          title="Meeting detection is unavailable"
          mb="md"
          role="alert"
        >
          <Text size="sm">{meetings.probe_error}</Text>
          <Text size="sm" c="dimmed" mt={6}>
            Articulate reads the running process list and Windows audio sessions
            to notice a meeting. Check that an audio input device is enabled,
            then turn an application off and on again to retry. You can still
            start a conversation manually.
          </Text>
        </Alert>
      )}
      <Paper withBorder radius="lg" className="secondary-settings-card">
        {meetingApps.map(([id, label, key]) => (
          <SettingRow key={id} title={label} description={appDescriptions[id]}>
            <Group gap="sm">
              {id === "meet" && (
                <Badge variant="light" color="gray">
                  Best effort
                </Badge>
              )}
              {active.some((item) => item.id === id) && (
                <Badge variant="light" color="mint">
                  In a meeting
                </Badge>
              )}
              {toggle(key, `Detect ${label} meetings`)}
            </Group>
          </SettingRow>
        ))}
        <SettingRow
          title="When a meeting is noticed"
          description={
            settings.meetings_start === "automatic"
              ? "A transcript starts by itself once a meeting is confirmed, with a notice that it began."
              : "Articulate asks before anything is recorded. Nothing starts until you choose."
          }
        >
          <SegmentedControl
            aria-label="When a meeting is noticed"
            data={[
              { value: "ask", label: "Ask first" },
              { value: "automatic", label: "Start automatically" },
            ]}
            value={settings.meetings_start || "ask"}
            onChange={(value) => patch("meetings_start", value)}
            disabled={busy}
          />
        </SettingRow>
        {prompt && (
          <SettingRow
            title={`${prompt.label} meeting detected`}
            description="Start a transcript for this meeting?"
          >
            <Group gap="xs">
              <Button
                size="sm"
                loading={pending === "meeting_respond"}
                disabled={busy || capturing}
                onClick={() => action("meeting_respond", { accept: true })}
              >
                Start transcript
              </Button>
              <Button
                size="sm"
                variant="subtle"
                color="gray"
                disabled={busy}
                onClick={() => action("meeting_respond", { accept: false })}
              >
                Not now
              </Button>
            </Group>
          </SettingRow>
        )}
        {meetings.consent_reminder && (
          <Text size="sm" c="dimmed" pt="lg" pb={meetings.enabled ? 0 : "lg"}>
            {meetings.consent_reminder}
          </Text>
        )}
        {meetings.enabled && (
          <SetupStatus
            message={
              meetings.owned
                ? `Transcribing the ${meetings.owned.label} meeting. It finishes and saves when the meeting ends.`
                : meetings.status || "Watching for a meeting."
            }
          />
        )}
        {events.length > 0 && (
          <div className="meeting-event-list" aria-label="Recent meetings">
            {events.map((event, index) => (
              <div
                key={`${event.app}-${event.at_unix_ms}-${index}`}
                style={{ display: "contents" }}
              >
                <time>{formatEventTime(event.at_unix_ms)}</time>
                <span>{event.label}</span>
                <span>{event.kind === "started" ? "Started" : "Ended"}</span>
              </div>
            ))}
          </div>
        )}
      </Paper>
    </SettingsSection>
  );
}
