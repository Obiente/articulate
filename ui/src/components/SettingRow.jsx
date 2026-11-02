import { Group, Loader, Progress, Text } from "@mantine/core";
import "../secondary.css";

export function SettingRow({ title, description, children }) {
  return (
    <div className="secondary-setting-row">
      <div>
        <Text fw={500}>{title}</Text>
        {description && (
          <Text c="dimmed" size="sm" mt={5}>
            {description}
          </Text>
        )}
      </div>
      <div className="secondary-setting-control">{children}</div>
    </div>
  );
}

export function SetupStatus({ message, error, busy, progress }) {
  if (!message && !error && !busy) return null;
  return (
    <div className="secondary-model-status" role={error ? "alert" : "status"}>
      <Group align="flex-start" gap="sm">
        {busy && <Loader size="xs" />}
        <Text size="sm" c={error ? "red" : "dimmed"}>
          {error || message || "Preparing…"}
        </Text>
      </Group>
      {busy && Number.isFinite(progress) && (
        <Progress
          mt="sm"
          value={Math.max(0, Math.min(100, progress * 100))}
          aria-label="Download progress"
        />
      )}
    </div>
  );
}

// A titled settings section with its card.
export function SettingsSection({ id, icon: Icon, title, children }) {
  return (
    <section id={id}>
      <div className="secondary-section-title">
        <Icon size={22} />
        <h2>{title}</h2>
      </div>
      {children}
    </section>
  );
}
