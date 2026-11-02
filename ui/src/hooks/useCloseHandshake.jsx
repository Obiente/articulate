// Closing the window: save pending edits, then ask the controller to close.
//
// The native shell sends `desktop-close-requested` with a request id. The
// window acknowledges it at once; without an acknowledgement the native
// shell closes on its own after a timeout. When the controller has stopped
// working and saves cannot be verified, the user may close anyway.
import { useCallback, useEffect, useState } from "react";
import { Button, Group, Modal, Text } from "@mantine/core";
import {
  acknowledgeClose,
  closeDesktop,
  controllerHealth,
  onCloseRequested,
} from "../bridge.js";

export function useCloseHandshake({ documents, report }) {
  const [unverified, setUnverified] = useState(null);
  const { flush, hasPending, inTransition, beginTransition, endTransition } =
    documents;

  // Returns whether the app is closing.
  const closeApp = useCallback(async () => {
    try {
      do {
        await flush();
      } while (hasPending());
      await closeDesktop();
      return true;
    } catch (e) {
      const health = await controllerHealth().catch(() => null);
      if (health?.can_close_anyway) setUnverified(String(e?.message ?? e));
      else report(e);
      return false;
    }
  }, [flush, hasPending, report]);

  useEffect(() => {
    let stopped = false,
      unlisten;
    onCloseRequested(async (id) => {
      // A document action is running; keep the window open.
      if (inTransition()) {
        void acknowledgeClose(id, true).catch(() => {});
        return;
      }
      await acknowledgeClose(id).catch(() => {});
      beginTransition();
      try {
        if (!(await closeApp()))
          await acknowledgeClose(id, true).catch(() => {});
      } finally {
        endTransition();
      }
    })
      .then((fn) => {
        if (stopped) fn();
        else unlisten = fn;
      })
      .catch(report);
    return () => {
      stopped = true;
      unlisten?.();
    };
  }, [closeApp, inTransition, beginTransition, endTransition, report]);

  const dialog = (
    <Modal
      opened={Boolean(unverified)}
      onClose={() => setUnverified(null)}
      title="Close without saving?"
      centered
      size="md"
    >
      <Text>
        Articulate stopped working, so your latest changes could not be verified
        as saved.
      </Text>
      {unverified && (
        <Text c="dimmed" size="sm" mt="sm">
          {unverified}
        </Text>
      )}
      <Group justify="flex-end" mt="xl">
        <Button variant="default" onClick={() => setUnverified(null)}>
          Keep Articulate open
        </Button>
        <Button
          color="red"
          onClick={() => void closeDesktop(true).catch(report)}
        >
          Close anyway
        </Button>
      </Group>
    </Modal>
  );
  return { closeApp, dialog };
}
