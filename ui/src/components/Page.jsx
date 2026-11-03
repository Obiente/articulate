import {
  Button,
  Group,
  Modal,
  Paper,
  Stack,
  Text,
  ThemeIcon,
} from "@mantine/core";
import "../secondary.css";

export function Heading({ title, children, action }) {
  return (
    <div className="secondary-heading">
      <div>
        <h1 className="page-heading">{title}</h1>
        <p className="page-subtitle">{children}</p>
      </div>
      {action}
    </div>
  );
}

export function Empty({ icon: Icon, title, children, action }) {
  return (
    <Paper className="secondary-empty" withBorder radius="lg">
      <ThemeIcon size={52} radius="xl" variant="light">
        <Icon size={25} />
      </ThemeIcon>
      <h2>{title}</h2>
      <Text c="dimmed" maw={430}>
        {children}
      </Text>
      {action}
    </Paper>
  );
}

export function DeleteDialog({ item, noun, onClose, onConfirm, busy }) {
  return (
    <Modal
      opened={Boolean(item)}
      onClose={onClose}
      title={`Delete ${noun}?`}
      centered
      radius="lg"
      size="sm"
    >
      <Stack gap="lg">
        <Text>
          Remove <strong>{item?.name}</strong> from your{" "}
          {noun === "word" ? "vocabulary" : "snippets"}? Saved transcripts will
          stay unchanged.
        </Text>
        <Group justify="flex-end">
          <Button variant="default" onClick={onClose} disabled={busy}>
            Keep {noun}
          </Button>
          <Button color="red" onClick={onConfirm} loading={busy}>
            Delete {noun}
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}

export function Metric({ label, value, description, icon: Icon }) {
  return (
    <Paper withBorder radius="lg" className="secondary-metric">
      <Group justify="space-between">
        <Text c="dimmed" size="sm" fw={500}>
          {label}
        </Text>
        <Icon size={21} className="secondary-accent" />
      </Group>
      <div className="secondary-metric-value">{value}</div>
      <Text c="dimmed" size="sm">
        {description}
      </Text>
    </Paper>
  );
}
