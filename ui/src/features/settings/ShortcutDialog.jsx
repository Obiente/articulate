import {
  Button,
  Checkbox,
  Group,
  Modal,
  Select,
  Stack,
  Text,
} from "@mantine/core";

const shortcutKeys = [
  "Space",
  ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ",
  ..."0123456789",
  ...Array.from({ length: 24 }, (_, index) => `F${index + 1}`),
  "Tab",
  "Enter",
  "Escape",
  "Backspace",
  "Delete",
  "Insert",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
];

// Edit the dictation or quick-note shortcut. `shortcut` is
// `{ target: "hotkey" | "quick_note_hotkey", value, open }`; it is kept
// after closing so the closing transition shows the same content.
export function ShortcutDialog({
  shortcut,
  pending,
  onChange,
  onClose,
  onSave,
}) {
  const quickNote = shortcut?.target === "quick_note_hotkey";
  const value = shortcut?.value;
  return (
    <Modal
      opened={Boolean(shortcut?.open)}
      onClose={() => !pending && onClose()}
      title={
        quickNote ? "Change quick note shortcut" : "Change keyboard shortcut"
      }
      centered
      radius="lg"
      size="md"
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void onSave();
        }}
      >
        <Stack gap="lg">
          <Text c="dimmed">
            {quickNote
              ? "Press once to start a spoken note from any app. Press again to finish and save it."
              : "Choose a combination that is comfortable to hold while you speak."}
          </Text>
          <Group gap="lg">
            {[
              ["ctrl", "Ctrl"],
              ["alt", "Alt"],
              ["shift", "Shift"],
              ["win", "Win"],
            ].map(([field, label]) => (
              <Checkbox
                key={field}
                label={label}
                checked={Boolean(value?.[field])}
                onChange={(event) =>
                  onChange({ ...value, [field]: event.currentTarget.checked })
                }
              />
            ))}
          </Group>
          <Select
            label="Key"
            data={shortcutKeys}
            searchable
            allowDeselect={false}
            value={value?.key || "Space"}
            onChange={(key) => key && onChange({ ...value, key })}
          />
          <Text size="sm" c="dimmed">
            Include Ctrl, Alt, or Win to keep ordinary typing available.
          </Text>
          <Group justify="flex-end">
            <Button
              variant="default"
              disabled={Boolean(pending)}
              onClick={onClose}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              loading={pending === shortcut?.target}
              disabled={!(value?.ctrl || value?.alt || value?.win)}
            >
              Save shortcut
            </Button>
          </Group>
        </Stack>
      </form>
    </Modal>
  );
}
