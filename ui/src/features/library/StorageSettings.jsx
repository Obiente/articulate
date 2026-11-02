import { useEffect, useState } from "react";
import {
  Alert,
  Button,
  Checkbox,
  Group,
  Modal,
  NumberInput,
  Paper,
  PasswordInput,
  Stack,
  Switch,
  Text,
} from "@mantine/core";
import { LockSimple } from "@phosphor-icons/react";
import {
  SettingRow,
  SettingsSection,
  SetupStatus,
} from "../../components/SettingRow.jsx";
import { formatDate } from "../../lib/format.js";
import { attentionCount, RecoveryDialog } from "./Recovery.jsx";
import "./library.css";
import { defaultPolicy, policyProblem, retentionStep } from "./retention.js";

const plural = (count, one, many) => `${count} ${count === 1 ? one : many}`;
const kindNames = {
  dictation: "Dictation",
  call: "Conversation",
  note: "Note",
};

function encryptionMessage(storage) {
  if (storage.encryption_working)
    return storage.encrypt_at_rest
      ? "Encrypting your saved items…"
      : "Decrypting your saved items…";
  const result = storage.encryption_result;
  if (!result || result.error) return undefined;
  const done = result.protect ? "Encrypted" : "Decrypted";
  if (result.failed)
    return `${done} ${plural(result.changed, "item", "items")}. ${plural(result.failed, "item", "items")} couldn't be changed yet and will be retried when Articulate restarts.`;
  return result.changed
    ? `${done} ${plural(result.changed, "item", "items")}.`
    : result.protect
      ? "Everything saved on this device is encrypted."
      : "Everything saved on this device is readable without encryption.";
}

function progressMessage(stage, choosing, working) {
  if (stage === "choosing") return choosing;
  if (stage === "working") return working;
  return undefined;
}

// Privacy & storage: encryption, retention, backups, exports and recovery.
export function StorageSettings({ controls }) {
  const { state, action, pending, busy, capturing } = controls;
  const storage = state.storage || {};
  const [backingUp, setBackingUp] = useState(false),
    [restoring, setRestoring] = useState(false),
    [reviewing, setReviewing] = useState(false);
  const fileOpen = storage.dialog_open;
  const operating =
    [storage.backup, storage.restore, storage.markdown].some(
      (stage) => stage && stage !== "idle",
    ) || fileOpen;
  const attention = attentionCount(state.recovery);
  const restore = storage.restore_result;
  const backup = storage.backup_result;
  const markdown = storage.markdown_result;
  const run = async (name, payload) => action(name, payload);
  return (
    <SettingsSection
      id="settings-storage"
      icon={LockSimple}
      title="Privacy & storage"
    >
      <Paper withBorder radius="lg" className="secondary-settings-card">
        <SettingRow
          title="Encrypt saved items"
          description="Lock your notes, transcripts and preferences to your Windows account, so only you can open them on this PC. Backups and exports are saved as you choose."
        >
          <Switch
            aria-label="Encrypt saved items"
            size="md"
            checked={Boolean(storage.encrypt_at_rest)}
            disabled={busy || storage.encryption_working}
            onChange={(event) =>
              run("storage_encryption", {
                enabled: event.currentTarget.checked,
              })
            }
          />
        </SettingRow>
        <SetupStatus
          busy={storage.encryption_working}
          message={encryptionMessage(storage)}
          error={storage.encryption_result?.error}
        />
        <RetentionEditor
          storage={storage}
          run={run}
          busy={busy}
          pending={pending}
        />
        <SettingRow
          title="Back up everything"
          description="Save your notes, conversations, dictations, vocabulary and preferences in one file. Models and your Discord pairing aren't included."
        >
          <Button
            variant="default"
            disabled={busy || operating}
            loading={storage.backup === "working"}
            onClick={() => setBackingUp(true)}
          >
            Create backup…
          </Button>
        </SettingRow>
        <SetupStatus
          busy={storage.backup === "working" || storage.backup === "choosing"}
          message={
            progressMessage(
              storage.backup,
              "Choose where to save your backup…",
              "Writing your backup…",
            ) ||
            (backup && !backup.error
              ? `Saved ${plural(backup.sessions, "item", "items")}${backup.encrypted ? " with a passphrase" : ""} to ${backup.path}.${backup.skipped ? ` ${plural(backup.skipped, "item", "items")} couldn't be read and weren't included.` : ""}`
              : undefined)
          }
          error={storage.backup === "idle" ? backup?.error : undefined}
        />
        <SettingRow
          title="Restore from a backup"
          description="Add the items from a backup to this device. Items here that are the same or newer are always kept."
        >
          <Button
            variant="default"
            disabled={busy || operating || capturing}
            loading={storage.restore === "working"}
            onClick={() => setRestoring(true)}
          >
            Restore…
          </Button>
        </SettingRow>
        <SetupStatus
          busy={storage.restore === "working" || storage.restore === "choosing"}
          message={
            progressMessage(
              storage.restore,
              "Choose a backup to restore…",
              "Checking and restoring your backup…",
            ) ||
            (restore && !restore.error ? restoreSummary(restore) : undefined)
          }
          error={storage.restore === "idle" ? restore?.error : undefined}
        />
        {storage.restore === "idle" && restore?.model_reload && (
          <Alert
            variant="light"
            color="mint"
            mx="lg"
            mb="md"
            title="Reload your speech model"
          >
            The restored preferences change how speech is recognized. Reload the
            model to use them.
            <Group mt="sm">
              <Button
                size="xs"
                variant="light"
                disabled={busy || capturing}
                loading={pending === "model_load"}
                onClick={() => run("model_load")}
              >
                Reload model
              </Button>
            </Group>
          </Alert>
        )}
        <SettingRow
          title="Export everything as Markdown"
          description="Write each item as its own Markdown file in a folder you choose. Existing files are never replaced."
        >
          <Button
            variant="default"
            disabled={busy || operating}
            loading={storage.markdown === "working"}
            onClick={() => run("history_export_markdown")}
          >
            Choose folder…
          </Button>
        </SettingRow>
        <SetupStatus
          busy={
            storage.markdown === "working" || storage.markdown === "choosing"
          }
          message={
            progressMessage(
              storage.markdown,
              "Choose a folder…",
              "Writing Markdown files…",
            ) ||
            (markdown && !markdown.error
              ? `Wrote ${plural(markdown.files, "file", "files")} to ${markdown.folder}.${markdown.skipped ? ` ${plural(markdown.skipped, "item", "items")} couldn't be read.` : ""}`
              : undefined)
          }
          error={storage.markdown === "idle" ? markdown?.error : undefined}
        />
        <SettingRow
          title="Damaged or unreadable items"
          description={
            attention
              ? `${plural(attention, "item needs", "items need")} your attention. Review, restore or export them.`
              : "Nothing needs attention. Items Articulate can't read are set aside here, never deleted."
          }
        >
          <Button
            variant="default"
            disabled={busy}
            onClick={() => setReviewing(true)}
          >
            Review
          </Button>
        </SettingRow>
      </Paper>
      <BackupDialog
        opened={backingUp}
        onClose={() => setBackingUp(false)}
        onCreate={async (passphrase) => {
          if (await run("backup_create", { passphrase: passphrase || null }))
            setBackingUp(false);
        }}
        busy={busy}
      />
      <RestoreDialog
        opened={restoring}
        onClose={() => setRestoring(false)}
        onRestore={async (options) => {
          if (await run("backup_restore", options)) setRestoring(false);
        }}
        busy={busy}
      />
      <RecoveryDialog
        opened={reviewing}
        onClose={() => setReviewing(false)}
        recovery={state.recovery}
        run={(command) => {
          const { type, ...payload } = command;
          return run(type, payload);
        }}
      />
    </SettingsSection>
  );
}

function restoreSummary(restore) {
  const parts = [
    restore.added || restore.updated
      ? `Restored ${plural(restore.added, "new item", "new items")}${restore.updated ? ` and updated ${plural(restore.updated, "item", "items")}` : ""} from ${plural(restore.sessions_in_archive, "item", "items")} in the backup.`
      : `Checked ${plural(restore.sessions_in_archive, "item", "items")} in the backup. Everything was already on this device.`,
  ];
  if (restore.newer_in_backup)
    parts.push(
      `${plural(restore.newer_in_backup, "item on this device is", "items on this device are")} older than the backup and ${restore.newer_in_backup === 1 ? "was" : "were"} kept. Restore again with “Replace older copies” to use the backup's version.`,
    );
  if (restore.deleted_locally)
    parts.push(
      `${plural(restore.deleted_locally, "item", "items")} you deleted on this device ${restore.deleted_locally === 1 ? "wasn't" : "weren't"} restored.`,
    );
  if (restore.failed)
    parts.push(
      `${plural(restore.failed, "item", "items")} couldn't be restored.`,
    );
  if (restore.settings_restored)
    parts.push(
      restore.shortcuts_changed
        ? "Preferences and vocabulary were restored, and your shortcuts were updated."
        : "Preferences and vocabulary were restored.",
    );
  return parts.join(" ");
}

function RetentionEditor({ storage, run, busy, pending }) {
  const saved = storage.retention || defaultPolicy;
  const [draft, setDraft] = useState(saved);
  // Follow saved changes made elsewhere, such as a restore.
  useEffect(() => setDraft(saved), [JSON.stringify(saved)]); // eslint-disable-line react-hooks/exhaustive-deps
  const preview = storage.retention_preview;
  const step = retentionStep(saved, draft, preview);
  const problem = policyProblem(draft);
  const set = (patch) => setDraft({ ...draft, ...patch });
  const total = preview?.total || 0;
  return (
    <>
      <SettingRow
        title="Remove old items"
        description="Delete items you haven't changed for a while. Pinned items, topics and the recordings filed in them, and anything open are always kept."
      >
        <Switch
          aria-label="Remove old items"
          size="md"
          checked={draft.enabled}
          disabled={busy}
          onChange={(event) => set({ enabled: event.currentTarget.checked })}
        />
      </SettingRow>
      <div className="storage-retention">
        <Group gap="lg" align="flex-end" wrap="wrap">
          <NumberInput
            label="Older than"
            suffix={draft.days === 1 ? " day" : " days"}
            min={1}
            max={36500}
            w={150}
            value={draft.days}
            disabled={busy || !draft.enabled}
            onChange={(value) => set({ days: Number(value) || 0 })}
          />
          <div role="group" aria-label="Applies to">
            <Text size="sm" fw={500}>
              Applies to
            </Text>
            <Group gap="md" mt={8} mb={8}>
              {[
                ["dictations", "Dictations"],
                ["calls", "Conversations"],
                ["notes", "Notes"],
              ].map(([key, label]) => (
                <Checkbox
                  key={key}
                  label={label}
                  checked={draft[key]}
                  disabled={busy || !draft.enabled}
                  onChange={(event) =>
                    set({ [key]: event.currentTarget.checked })
                  }
                />
              ))}
            </Group>
          </div>
        </Group>
        {problem && step !== "saved" && (
          <Text c="red" size="sm" mt="sm">
            {problem}
          </Text>
        )}
        {step === "confirm" && (
          <div className="retention-preview" role="region" aria-live="polite">
            <Text fw={500} size="sm">
              {total
                ? `${plural(total, "item", "items")} will be deleted now`
                : "Nothing would be deleted now"}
            </Text>
            <Text c="dimmed" size="sm">
              {total
                ? "Deleted items can't be recovered unless you have a backup. Later items are removed automatically once a day."
                : "Items are removed automatically once they're old enough."}
            </Text>
            {total > 0 && (
              <ul className="retention-list">
                {preview.items.map((item) => (
                  <li key={item.id}>
                    <span>{item.title || "Untitled"}</span>
                    <span>
                      {kindNames[item.kind]} · last changed{" "}
                      {formatDate(item.updated_ms)}
                    </span>
                  </li>
                ))}
                {total > preview.items.length && (
                  <li className="muted">
                    and {total - preview.items.length} more
                  </li>
                )}
              </ul>
            )}
          </div>
        )}
        {preview?.error && !storage.retention_preview_working && (
          <Text c="red" size="sm" mt="sm">
            {preview.error}
          </Text>
        )}
        <Group gap="sm" mt="md" justify="flex-end">
          {step !== "saved" && (
            <Button
              variant="subtle"
              color="gray"
              disabled={busy}
              onClick={() => setDraft(saved)}
            >
              Cancel
            </Button>
          )}
          {step === "save" && (
            <Button
              disabled={busy}
              loading={pending === "retention_save"}
              onClick={() => run("retention_save", { policy: draft })}
            >
              Save
            </Button>
          )}
          {step === "preview" && (
            <Button
              disabled={busy || storage.retention_preview_working}
              loading={
                pending === "retention_preview" ||
                storage.retention_preview_working
              }
              onClick={() => run("retention_preview", { policy: draft })}
            >
              Review what's removed
            </Button>
          )}
          {step === "confirm" && (
            <Button
              color={total ? "red" : undefined}
              disabled={busy}
              loading={pending === "retention_save"}
              onClick={() => run("retention_save", { policy: draft })}
            >
              {total
                ? `Delete ${plural(total, "item", "items")} and turn on`
                : "Turn on"}
            </Button>
          )}
        </Group>
        {step === "saved" && saved.enabled && (
          <Text c="dimmed" size="sm" mt="xs">
            On: items older than {plural(saved.days, "day", "days")} are removed
            when Articulate starts and once a day.
          </Text>
        )}
      </div>
    </>
  );
}

function BackupDialog({ opened, onClose, onCreate, busy }) {
  const [passphrase, setPassphrase] = useState(""),
    [confirm, setConfirm] = useState("");
  useEffect(() => {
    if (!opened) {
      setPassphrase("");
      setConfirm("");
    }
  }, [opened]);
  const short = passphrase && [...passphrase].length < 8;
  const mismatch = passphrase && confirm !== passphrase;
  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title="Back up everything"
      centered
    >
      <Stack gap="md">
        <Text c="dimmed" size="sm">
          Add a passphrase to encrypt the backup. You'll need it to restore, and
          it can't be recovered if you forget it. Without one, anyone with the
          file can read it.
        </Text>
        <PasswordInput
          label="Passphrase (optional)"
          description="At least 8 characters"
          value={passphrase}
          onChange={(event) => setPassphrase(event.currentTarget.value)}
          error={short ? "Use at least 8 characters." : null}
          autoComplete="new-password"
        />
        {passphrase && (
          <PasswordInput
            label="Confirm passphrase"
            value={confirm}
            onChange={(event) => setConfirm(event.currentTarget.value)}
            error={confirm && mismatch ? "The passphrases don't match." : null}
            autoComplete="new-password"
          />
        )}
        <Group justify="flex-end">
          <Button variant="default" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={busy || short || mismatch}
            onClick={() => onCreate(passphrase)}
          >
            Choose where to save
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}

function RestoreDialog({ opened, onClose, onRestore, busy }) {
  const [passphrase, setPassphrase] = useState(""),
    [replaceOlder, setReplaceOlder] = useState(false),
    [includeSettings, setIncludeSettings] = useState(false);
  useEffect(() => {
    if (!opened) setPassphrase("");
  }, [opened]);
  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title="Restore from a backup"
      centered
    >
      <Stack gap="md">
        <Text c="dimmed" size="sm">
          The whole backup is checked before anything changes. Items you deleted
          on this device stay deleted.
        </Text>
        <PasswordInput
          label="Passphrase"
          description="Only if the backup has one"
          value={passphrase}
          onChange={(event) => setPassphrase(event.currentTarget.value)}
          autoComplete="current-password"
        />
        <Checkbox
          label="Replace older copies"
          description="When an item here is older than the backup's copy, use the backup's version. Otherwise those items are listed and left as they are."
          checked={replaceOlder}
          onChange={(event) => setReplaceOlder(event.currentTarget.checked)}
        />
        <Checkbox
          label="Include preferences and vocabulary"
          description="Restore shortcuts, settings, vocabulary and snippets. Encryption and old-item removal stay as they are on this device."
          checked={includeSettings}
          onChange={(event) => setIncludeSettings(event.currentTarget.checked)}
        />
        <Group justify="flex-end">
          <Button variant="default" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={busy}
            onClick={() =>
              onRestore({
                passphrase: passphrase || null,
                replace_older: replaceOlder,
                include_settings: includeSettings,
              })
            }
          >
            Choose backup
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
