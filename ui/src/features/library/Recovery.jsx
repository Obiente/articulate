import { useEffect, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Group,
  Loader,
  Modal,
  Stack,
  Text,
  Tooltip,
} from "@mantine/core";
import { ArrowRight, Warning } from "@phosphor-icons/react";
import { formatDate } from "../../lib/format.js";
import { attentionCount } from "./attention.js";

export { attentionCount };

const sizeLabel = (bytes) =>
  bytes >= 1024 * 1024
    ? `${(bytes / 1024 / 1024).toFixed(1)} MB`
    : `${Math.max(1, Math.round(bytes / 1024))} KB`;

export function RecoveryBanner({ recovery, onReview }) {
  const count = attentionCount(recovery);
  if (!count) return null;
  return (
    <button className="attention-banner" onClick={onReview}>
      <Warning size={18} weight="duotone" />
      <span>
        {count} {count === 1 ? "item needs" : "items need"} attention
      </span>
      <span>
        Review <ArrowRight size={15} />
      </span>
    </button>
  );
}

// Review isolated files and unreadable records. `run(command)` sends an
// action and resolves to whether it succeeded.
export function RecoveryDialog({ opened, onClose, recovery, run }) {
  const [discarding, setDiscarding] = useState(null),
    [pending, setPending] = useState("");
  useEffect(() => {
    if (opened) void run({ type: "quarantine_list" });
    else setDiscarding(null);
  }, [opened]); // eslint-disable-line react-hooks/exhaustive-deps
  const files = recovery?.quarantine || [];
  const warnings = (recovery?.warnings || []).filter(
    (warning) => warning.kind !== "damaged",
  );
  const unavailable = warnings.filter((w) => w.kind === "unavailable");
  const newer = warnings.filter((w) => w.kind === "newer_version");
  const act = async (name, command) => {
    setPending(name);
    try {
      return await run(command);
    } finally {
      setPending("");
    }
  };
  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title="Items that need attention"
      size="lg"
      centered
    >
      <Stack gap="lg">
        <Text c="dimmed" size="sm">
          Articulate sets aside saved items it can't read, so the rest of your
          library keeps working. Nothing here is deleted unless you choose to.
        </Text>
        <section aria-labelledby="recovery-isolated">
          <Group justify="space-between" mb="xs">
            <Text fw={600} id="recovery-isolated">
              Damaged items
            </Text>
            {!recovery?.listed && opened && <Loader size="xs" />}
          </Group>
          {files.length ? (
            <Stack gap="sm">
              {files.map((file) => (
                <div className="recovery-item" key={file.file_name}>
                  <div className="recovery-item-copy">
                    <Text fw={500}>{file.title || "Untitled item"}</Text>
                    <Text size="xs" c="dimmed">
                      {formatDate(file.modified_ms)} · {sizeLabel(file.bytes)}
                      {file.restorable ? "" : " · Can't be restored"}
                    </Text>
                    <Text size="xs" c="dimmed">
                      {file.reason}
                    </Text>
                  </div>
                  {discarding === file.file_name ? (
                    <Group gap="xs" wrap="nowrap">
                      <Button
                        size="xs"
                        variant="default"
                        onClick={() => setDiscarding(null)}
                        disabled={Boolean(pending)}
                      >
                        Keep
                      </Button>
                      <Button
                        size="xs"
                        color="red"
                        loading={pending === `discard:${file.file_name}`}
                        onClick={async () => {
                          await act(`discard:${file.file_name}`, {
                            type: "quarantine_discard",
                            file_name: file.file_name,
                          });
                          setDiscarding(null);
                        }}
                      >
                        Delete permanently
                      </Button>
                    </Group>
                  ) : (
                    <Group gap="xs" wrap="nowrap">
                      <Tooltip
                        label="This file doesn't hold a complete item. Export it to keep a copy."
                        disabled={file.restorable}
                        multiline
                        w={240}
                      >
                        <Button
                          size="xs"
                          variant="light"
                          disabled={!file.restorable || Boolean(pending)}
                          loading={pending === `restore:${file.file_name}`}
                          onClick={() =>
                            act(`restore:${file.file_name}`, {
                              type: "quarantine_restore",
                              file_name: file.file_name,
                            })
                          }
                        >
                          Restore
                        </Button>
                      </Tooltip>
                      <Button
                        size="xs"
                        variant="default"
                        disabled={Boolean(pending)}
                        onClick={() =>
                          act(`export:${file.file_name}`, {
                            type: "quarantine_export",
                            file_name: file.file_name,
                          })
                        }
                      >
                        Export…
                      </Button>
                      <Button
                        size="xs"
                        variant="subtle"
                        color="red"
                        disabled={Boolean(pending)}
                        onClick={() => setDiscarding(file.file_name)}
                      >
                        Discard
                      </Button>
                    </Group>
                  )}
                </div>
              ))}
            </Stack>
          ) : (
            <Text size="sm" c="dimmed">
              {recovery?.listed || !opened
                ? "No damaged items."
                : "Looking for damaged items…"}
            </Text>
          )}
          {files.length > 0 && (
            <Text size="xs" c="dimmed" mt="sm">
              Restoring never replaces a newer copy of the same item. Exports
              are saved as readable files, even when encryption is on.
            </Text>
          )}
        </section>
        {unavailable.length > 0 && (
          <Alert variant="light" color="yellow" title="Temporarily unavailable">
            {unavailable.length} {unavailable.length === 1 ? "item" : "items"}{" "}
            couldn't be opened right now, often because another app such as
            antivirus or backup software is using the file. Articulate tries
            again automatically and never sets these aside.
          </Alert>
        )}
        {newer.length > 0 && (
          <Alert variant="light" color="gray" title="Saved by a newer version">
            {newer.length} {newer.length === 1 ? "item was" : "items were"}{" "}
            saved by a newer version of Articulate. Update Articulate to open{" "}
            {newer.length === 1 ? "it" : "them"}. They're left untouched.
          </Alert>
        )}
        <Group justify="space-between">
          <Badge variant="light" color={files.length ? "yellow" : "gray"}>
            {files.length} set aside
          </Badge>
          <Button variant="default" onClick={onClose}>
            Done
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
