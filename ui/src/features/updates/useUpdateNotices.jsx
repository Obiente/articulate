// Checks for app updates and shows update notices once each.
import { useEffect, useMemo, useRef } from "react";
import { Button } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { native } from "../../bridge.js";
import { createUpdateNoticeTracker, updateNotices } from "./update-notices.js";

import { shouldCheckUpdates } from "./update-policy.js";

const DAY = 24 * 60 * 60 * 1000;

export function useUpdateNotices({ state, act, showSettingsSection }) {
  const tracker = useRef(createUpdateNoticeTracker()),
    checked = useRef(false),
    updaterState = useRef("idle"),
    openSection = useRef(showSettingsSection);
  updaterState.current = state?.updater?.state || "idle";
  openSection.current = showSettingsSection;
  const notices = useMemo(
    () => updateNotices(state),
    [state?.preview, state?.updater, state?.discord],
  );

  // Check once at startup, then daily while the window stays open.
  const ready = Boolean(state?.settings);
  const enabled = state?.settings?.automatic_update_checks === true;
  useEffect(() => {
    if (!native || !ready || !enabled || checked.current) return;
    checked.current = true;
    if (updaterState.current === "idle")
      void act({ type: "update_check" }).catch(() => {});
  }, [ready, enabled, act]);
  useEffect(() => {
    if (!native || !enabled) return;
    const interval = setInterval(() => {
      if (shouldCheckUpdates(enabled, updaterState.current))
        void act({ type: "update_check" }).catch(() => {});
    }, DAY);
    return () => clearInterval(interval);
  }, [act, enabled]);

  useEffect(() => {
    for (const notice of tracker.current(notices)) {
      notifications.show({
        id: `update-${notice.id}`,
        title: notice.title,
        message: (
          <div>
            <span>{notice.message}</span>
            <Button
              variant="subtle"
              size="compact-sm"
              mt="sm"
              onClick={() => openSection.current(notice.section)}
            >
              Open settings
            </Button>
          </div>
        ),
        color: notice.color,
        autoClose: notice.color === "red" ? 12000 : 9000,
        role: "status",
        "aria-live": "polite",
      });
    }
  }, [notices]);
  return notices;
}
