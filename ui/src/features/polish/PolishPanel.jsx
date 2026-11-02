import { useState } from "react";
import {
  Alert,
  Button,
  Group,
  Modal,
  Progress,
  Stack,
  Text,
} from "@mantine/core";
import { textDiff } from "./diff.js";
export function PolishPanel({ state, act }) {
  const [opened, setOpened] = useState(false),
    cleanup = state.cleanup || {},
    preview = cleanup.preview;
  const run = (type) => void act({ type }).catch(() => {});
  const close = () => {
    run("polish_cancel");
    setOpened(false);
  };
  const diff = preview && textDiff(preview.source, preview.text);
  return (
    <>
      <Button
        size="xs"
        variant="light"
        disabled={!state.text || state.recording || state.busy}
        onClick={() => {
          setOpened(true);
          if (cleanup.installed) run("polish_start");
        }}
      >
        Polish
      </Button>
      <Modal
        opened={opened}
        onClose={close}
        title="Polish your words"
        size="lg"
        centered
      >
        <Stack>
          {!cleanup.installed ? (
            <>
              <Text>
                Download the local cleanup model to preview edits on this
                computer.
              </Text>
              <Button
                loading={cleanup.working}
                onClick={() => run("cleanup_download")}
              >
                Download cleanup model
              </Button>
            </>
          ) : (
            !cleanup.working &&
            !preview && (
              <Button onClick={() => run("polish_start")}>
                Create preview
              </Button>
            )
          )}
          <Text role="status">{cleanup.status}</Text>
          {cleanup.working && (
            <Progress
              aria-label="Polish progress"
              animated
              value={(cleanup.progress || 0) * 100}
            />
          )}
          {preview && (
            <>
              <Text fw={600}>Safe baseline</Text>
              <Text style={{ whiteSpace: "pre-wrap" }}>{preview.source}</Text>
              <Text fw={600}>Proposed edit</Text>
              <Text style={{ whiteSpace: "pre-wrap" }}>
                {diff.prefix}
                <del>{diff.removed}</del>
                <ins>{diff.added}</ins>
                {diff.suffix}
              </Text>
              {preview.blocked && (
                <Alert
                  color="yellow"
                  title={
                    preview.cleanup_only
                      ? "Speech cleanup only"
                      : "Edit blocked"
                  }
                >
                  {preview.blocked}
                </Alert>
              )}
              <Group>
                <Button
                  disabled={Boolean(preview.blocked && !preview.cleanup_only)}
                  onClick={() => {
                    run("polish_apply");
                    setOpened(false);
                  }}
                >
                  Apply
                </Button>
                <Button variant="default" onClick={close}>
                  Keep original
                </Button>
              </Group>
            </>
          )}
          <Button variant="subtle" onClick={close}>
            Cancel
          </Button>
        </Stack>
      </Modal>
    </>
  );
}
