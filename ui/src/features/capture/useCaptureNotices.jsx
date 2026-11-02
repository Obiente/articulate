// Confirms native call and spoken-note capture starts, stops and failures.
import { useEffect, useRef } from "react";
import { Button } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { Record, Stop, X } from "@phosphor-icons/react";
import { createCaptureFeedbackTracker } from "./capture-feedback.js";

export function useCaptureNotices({ feedback, openRecording }) {
  const tracker = useRef(createCaptureFeedbackTracker()),
    open = useRef(openRecording);
  open.current = openRecording;
  useEffect(() => {
    const notice = tracker.current(feedback);
    if (!notice) return;
    notifications.show({
      id: `capture-feedback-${notice.sequence}`,
      title: notice.title,
      message: (
        <div>
          <span>{notice.message}</span>
          {notice.sessionId && (
            <Button
              variant="subtle"
              size="compact-sm"
              mt="sm"
              onClick={() => void open.current?.(notice.sessionId)}
            >
              Open recording
            </Button>
          )}
        </div>
      ),
      icon:
        notice.event === "started" ? (
          <Record size={18} weight="fill" />
        ) : notice.event === "failed" ? (
          <X size={18} />
        ) : (
          <Stop size={18} weight="fill" />
        ),
      color: notice.event === "failed" ? "red" : "mint",
      autoClose: notice.event === "failed" ? 10000 : 5500,
      role: "status",
      "aria-live": "polite",
      "aria-atomic": true,
      closeButtonProps: { "aria-label": "Dismiss recording confirmation" },
    });
  }, [feedback]);
}
