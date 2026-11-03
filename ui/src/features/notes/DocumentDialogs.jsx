import { useEffect, useRef, useState } from "react";
import { Button, Group, Modal, Select } from "@mantine/core";
import { sourceMoveAction } from "../../actions.js";

const SEPARATE = "__separate__";

// Move a dictation into a topic, or keep it as a separate note.
export function MoveSourceDialog({ source, topics, blocked, act, onClose }) {
  const [target, setTarget] = useState(SEPARATE),
    [pending, setPending] = useState(false);
  useEffect(() => {
    if (source) setTarget(source.topic_id || SEPARATE);
  }, [source]);
  return (
    <Modal
      opened={Boolean(source)}
      onClose={() => !pending && onClose()}
      title="Move this dictation"
      centered
      size="md"
    >
      <p className="muted">
        Move “{source?.title}” while keeping its original words. The topic notes
        will update to reflect their remaining dictations.
      </p>
      <Select
        label="Topic"
        searchable
        allowDeselect={false}
        data={[
          { value: SEPARATE, label: "Keep as a separate note" },
          ...topics.map((topic) => ({ value: topic.id, label: topic.title })),
        ]}
        value={target}
        onChange={(value) => setTarget(value ?? SEPARATE)}
        disabled={pending}
        nothingFoundMessage="No matching topics"
      />
      {blocked && (
        <p className="muted">
          Moving is available when recording and note updates finish.
        </p>
      )}
      <Group justify="flex-end" mt="xl">
        <Button variant="default" disabled={pending} onClick={onClose}>
          Cancel
        </Button>
        <Button
          loading={pending}
          disabled={blocked || target === (source?.topic_id || SEPARATE)}
          onClick={async () => {
            setPending(true);
            try {
              await act(
                sourceMoveAction(
                  source.id,
                  target === SEPARATE ? null : target,
                  source.topic_id,
                ),
              );
              onClose();
            } catch {
              // The shell displays the error.
            } finally {
              setPending(false);
            }
          }}
        >
          Move dictation
        </Button>
      </Group>
    </Modal>
  );
}

export function DeleteDocumentDialog({ opened, noun, onClose, onConfirm }) {
  const [pending, setPending] = useState(false);
  const busy = useRef(false);
  const confirm = async () => {
    if (busy.current) return;
    busy.current = true;
    setPending(true);
    try {
      await onConfirm();
    } finally {
      busy.current = false;
      setPending(false);
    }
  };
  return (
    <Modal
      opened={opened}
      onClose={() => !busy.current && onClose()}
      title={`Delete this ${noun}?`}
      size="sm"
    >
      <p>The saved transcript and notes will be removed from this device.</p>
      <Group justify="flex-end" mt="xl">
        <Button variant="default" disabled={pending} onClick={onClose}>
          Keep {noun}
        </Button>
        <Button
          color="red"
          loading={pending}
          disabled={pending}
          onClick={confirm}
        >
          Delete {noun}
        </Button>
      </Group>
    </Modal>
  );
}
