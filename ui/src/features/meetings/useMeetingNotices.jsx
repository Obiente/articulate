// A non-intrusive prompt when a meeting is detected and mode is "ask", and
// a small indicator while a transcript started by detection is running.
import { useEffect, useMemo, useRef } from "react";
import { Button, Group, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { Record, VideoCamera } from "@phosphor-icons/react";
import {
  createMeetingPromptTracker,
  meetingIndicator,
  meetingPrompt,
} from "./meeting-notices.js";
import "../discord/participants.css";

export function useMeetingNotices({ state, act }) {
  const tracker = useRef(createMeetingPromptTracker());
  const respond = useRef(act);
  respond.current = act;
  const prompt = useMemo(() => meetingPrompt(state), [state?.meetings]);
  useEffect(() => {
    const { show, hide } = tracker.current(prompt);
    if (hide) notifications.hide(hide);
    if (!show) return;
    const answer = (accept) => {
      notifications.hide(show.id);
      void respond.current({ type: "meeting_respond", accept }).catch(() => {});
    };
    notifications.show({
      id: show.id,
      title: `${show.label} meeting detected`,
      message: (
        <div>
          <Text size="sm">
            Start a transcript for this meeting?
            {show.bestEffort
              ? " Google Meet is recognised best-effort through a browser using the microphone."
              : ""}
          </Text>
          {show.consent && (
            <Text size="xs" c="dimmed" mt={6}>
              {show.consent}
            </Text>
          )}
          <Group gap="xs" mt="sm">
            <Button size="compact-sm" onClick={() => answer(true)}>
              Start transcript
            </Button>
            <Button
              size="compact-sm"
              variant="subtle"
              color="gray"
              onClick={() => answer(false)}
            >
              Not now
            </Button>
          </Group>
        </div>
      ),
      icon: <VideoCamera size={18} />,
      color: "mint",
      autoClose: false,
      withCloseButton: false,
      role: "status",
      "aria-live": "polite",
    });
  }, [prompt]);
  return useMemo(() => meetingIndicator(state), [state?.meetings]);
}

// Shown at the top of the content area while detection owns a transcript.
export function MeetingIndicator({ indicator }) {
  if (!indicator) return null;
  return (
    <div className="meeting-indicator" role="status">
      <Record size={16} weight="fill" />
      <span>{indicator.title}</span>
      {indicator.status && <small>{indicator.status}</small>}
    </div>
  );
}
