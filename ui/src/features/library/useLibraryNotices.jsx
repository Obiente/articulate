import { useEffect, useRef } from "react";
import { notifications } from "@mantine/notifications";
import { Check } from "@phosphor-icons/react";

// One toast per library notice (exports, restores, retention). The first
// snapshot is a baseline, so reopening the window doesn't repeat old notices.
export function useLibraryNotices(notice) {
  const seen = useRef(undefined);
  useEffect(() => {
    if (notice === undefined) return;
    const sequence = notice?.sequence ?? 0;
    if (seen.current === undefined) {
      seen.current = sequence;
      return;
    }
    if (sequence <= seen.current) return;
    seen.current = sequence;
    notifications.show(
      notice.error
        ? { title: "Could not complete that", message: notice.message, color: "red" }
        : {
            message: notice.message,
            icon: <Check size={18} />,
            autoClose: 3200,
          },
    );
  }, [notice]);
}
