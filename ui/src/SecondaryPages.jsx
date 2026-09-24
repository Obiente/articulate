import { useEffect, useState } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Button,
  Checkbox,
  Divider,
  Group,
  Loader,
  Modal,
  Paper,
  Progress,
  SegmentedControl,
  Select,
  SimpleGrid,
  Stack,
  Switch,
  Table,
  Text,
  Textarea,
  TextInput,
  ThemeIcon,
  Tooltip,
} from "@mantine/core";
import {
  ArrowClockwise,
  ArrowRight,
  BookOpen,
  ChartBar,
  Check,
  DownloadSimple,
  Keyboard,
  Microphone,
  PencilSimple,
  Plus,
  Scissors,
  ShieldCheck,
  Sparkle,
  Trash,
  MagnifyingGlass,
  Waveform,
} from "@phosphor-icons/react";
import "./secondary.css";

const count = (value) => Number(value || 0).toLocaleString();
const emptyEntry = { heard: "", wanted: "", app: "", cues: "" };
const emptySnippet = { trigger: "", expansion: "", app: "" };
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

function Heading({ title, children, action }) {
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

function Empty({ icon: Icon, title, children, action }) {
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

function DeleteDialog({ item, noun, onClose, onConfirm, busy }) {
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

export function VocabularyPage({ state, act }) {
  const [search, setSearch] = useState("");
  const [editor, setEditor] = useState(null);
  const [deletion, setDeletion] = useState(null);
  const [busy, setBusy] = useState(false);
  const entries = state.dictionary || [];
  const filtered = entries
    .map((entry, index) => ({ ...entry, index }))
    .filter((entry) =>
      `${entry.heard} ${entry.wanted} ${entry.app || ""} ${(entry.cues || []).join(" ")}`
        .toLowerCase()
        .includes(search.toLowerCase()),
    );
  const add = () => setEditor({ ...emptyEntry });
  const save = async (event) => {
    event.preventDefault();
    setBusy(true);
    try {
      await act({ type: "dictionary_save", ...editor });
      setEditor(null);
    } catch {
      /* Root presents the actionable error. */
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    setBusy(true);
    try {
      await act({
        type: "dictionary_delete",
        index: deletion.index,
        expected_heard: deletion.name,
        expected_entry: deletion.original,
      });
      setDeletion(null);
    } catch {
      /* Root presents the error. */
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="secondary-page">
      <Heading
        title="Vocabulary"
        action={
          <Button leftSection={<Plus size={18} />} onClick={add}>
            Add a word
          </Button>
        }
      >
        Your names, terms, and spellings. Remembered wherever you dictate.
      </Heading>
      <Paper className="secondary-tip" withBorder radius="lg">
        <ThemeIcon size={42} radius="md" variant="light">
          <BookOpen size={23} />
        </ThemeIcon>
        <div>
          <Text fw={600}>Make it sound like you</Text>
          <Text c="dimmed" size="sm">
            Add an unusual name or a phrase you often correct. Adding the same
            correction updates its context.
          </Text>
        </div>
      </Paper>
      <div className="secondary-list-toolbar">
        <TextInput
          aria-label="Search vocabulary"
          placeholder="Find a word or spelling"
          leftSection={<MagnifyingGlass size={18} />}
          value={search}
          onChange={(event) => setSearch(event.currentTarget.value)}
        />
        <Text c="dimmed" size="sm">
          {count(entries.length)} {entries.length === 1 ? "word" : "words"}
        </Text>
      </div>
      {filtered.length ? (
        <Paper withBorder radius="lg" className="secondary-table">
          <Table.ScrollContainer minWidth={610}>
            <Table verticalSpacing="lg" horizontalSpacing="lg">
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>When you say</Table.Th>
                  <Table.Th>Write instead</Table.Th>
                  <Table.Th>Where it works</Table.Th>
                  <Table.Th>
                    <span className="secondary-sr-only">Actions</span>
                  </Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {filtered.map((entry) => (
                  <Table.Tr key={entry.index}>
                    <Table.Td>
                      <Text fw={500}>{entry.heard}</Text>
                      {entry.enabled === false && (
                        <Badge size="xs" variant="light" color="gray">
                          Off
                        </Badge>
                      )}
                    </Table.Td>
                    <Table.Td>
                      <Text className="secondary-accent">{entry.wanted}</Text>
                      {entry.cues?.length > 0 && (
                        <Text c="dimmed" size="xs" mt={5}>
                          Near: {entry.cues.join(", ")}
                        </Text>
                      )}
                    </Table.Td>
                    <Table.Td>
                      <Badge variant="light" color="gray">
                        {entry.app || "All apps"}
                      </Badge>
                    </Table.Td>
                    <Table.Td>
                      <Group gap={6} justify="flex-end" wrap="nowrap">
                        <Tooltip label="Edit word">
                          <ActionIcon
                            size={38}
                            variant="subtle"
                            aria-label={`Edit ${entry.heard}`}
                            onClick={() =>
                              setEditor({
                                heard: entry.heard,
                                wanted: entry.wanted,
                                app: entry.app || "",
                                cues: (entry.cues || []).join(", "),
                                index: entry.index,
                                expected_heard: entry.heard,
                                expected_entry: structuredClone(
                                  entries[entry.index],
                                ),
                              })
                            }
                          >
                            <PencilSimple size={19} />
                          </ActionIcon>
                        </Tooltip>
                        <Tooltip label="Delete word">
                          <ActionIcon
                            size={38}
                            variant="subtle"
                            color="gray"
                            aria-label={`Delete ${entry.heard}`}
                            onClick={() =>
                              setDeletion({
                                index: entry.index,
                                name: entry.heard,
                                original: structuredClone(entries[entry.index]),
                              })
                            }
                          >
                            <Trash size={19} />
                          </ActionIcon>
                        </Tooltip>
                      </Group>
                    </Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        </Paper>
      ) : (
        <Empty
          icon={BookOpen}
          title={search ? "No matching words" : "A vocabulary that knows you"}
          action={
            !search && (
              <Button
                variant="light"
                leftSection={<Plus size={17} />}
                onClick={add}
              >
                Add your first word
              </Button>
            )
          }
        >
          {search
            ? "Try a different word, spelling, or app."
            : "Give names, acronyms, and specialist terms the spelling they deserve."}
        </Empty>
      )}
      <Modal
        opened={Boolean(editor)}
        onClose={() => !busy && setEditor(null)}
        title={editor?.index == null ? "Add a word" : "Edit word"}
        centered
        size="lg"
        radius="lg"
      >
        <form onSubmit={save}>
          <Stack gap="lg">
            <SimpleGrid cols={{ base: 1, sm: 2 }}>
              <TextInput
                label="When you say"
                placeholder="For example, articulate"
                required
                autoFocus
                value={editor?.heard || ""}
                onChange={(event) =>
                  setEditor({ ...editor, heard: event.currentTarget.value })
                }
              />
              <TextInput
                label="Write instead"
                placeholder="Articulate"
                required
                value={editor?.wanted || ""}
                onChange={(event) =>
                  setEditor({ ...editor, wanted: event.currentTarget.value })
                }
              />
            </SimpleGrid>
            <TextInput
              label="App"
              description="Leave empty to use this spelling in every app."
              placeholder="All apps, or an app such as discord.exe"
              value={editor?.app || ""}
              onChange={(event) =>
                setEditor({ ...editor, app: event.currentTarget.value })
              }
            />
            <TextInput
              label="Context words"
              description="Optional. Use this spelling near these words in the same sentence."
              placeholder="Separate words with commas"
              value={editor?.cues || ""}
              onChange={(event) =>
                setEditor({ ...editor, cues: event.currentTarget.value })
              }
            />
            <Group justify="flex-end" mt="sm">
              <Button
                variant="default"
                onClick={() => setEditor(null)}
                disabled={busy}
              >
                Cancel
              </Button>
              <Button type="submit" loading={busy}>
                Save word
              </Button>
            </Group>
          </Stack>
        </form>
      </Modal>
      <DeleteDialog
        item={deletion}
        noun="word"
        onClose={() => !busy && setDeletion(null)}
        onConfirm={remove}
        busy={busy}
      />
    </div>
  );
}

export function SnippetsPage({ state, act }) {
  const [search, setSearch] = useState("");
  const [editor, setEditor] = useState(null);
  const [deletion, setDeletion] = useState(null);
  const [busy, setBusy] = useState(false);
  const snippets = state.macros || [];
  const filtered = snippets
    .map((snippet, index) => ({ ...snippet, index }))
    .filter((snippet) =>
      `${snippet.trigger} ${snippet.expansion} ${snippet.app || ""}`
        .toLowerCase()
        .includes(search.toLowerCase()),
    );
  const add = () => setEditor({ ...emptySnippet });
  const save = async (event) => {
    event.preventDefault();
    setBusy(true);
    try {
      await act({ type: "macro_save", ...editor });
      setEditor(null);
    } catch {
      /* Root presents the error. */
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    setBusy(true);
    try {
      await act({
        type: "macro_delete",
        index: deletion.index,
        expected_trigger: deletion.name,
        expected_macro: deletion.original,
      });
      setDeletion(null);
    } catch {
      /* Root presents the error. */
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="secondary-page">
      <Heading
        title="Snippets"
        action={
          <Button leftSection={<Plus size={18} />} onClick={add}>
            New snippet
          </Button>
        }
      >
        Say a little. Write a lot. Keep your everyday phrases within reach.
      </Heading>
      <Paper className="secondary-tip" withBorder radius="lg">
        <ThemeIcon size={42} radius="md" variant="light">
          <Scissors size={23} />
        </ThemeIcon>
        <div>
          <Text fw={600}>Your words, on cue</Text>
          <Text c="dimmed" size="sm">
            Say “bang” followed by a snippet name to insert its text. Useful for
            signatures, addresses, and familiar replies.
          </Text>
        </div>
      </Paper>
      <div className="secondary-list-toolbar">
        <TextInput
          aria-label="Search snippets"
          placeholder="Find a snippet"
          leftSection={<MagnifyingGlass size={18} />}
          value={search}
          onChange={(event) => setSearch(event.currentTarget.value)}
        />
        <Text c="dimmed" size="sm">
          {count(snippets.length)}{" "}
          {snippets.length === 1 ? "snippet" : "snippets"}
        </Text>
      </div>
      {filtered.length ? (
        <Stack gap="sm">
          {filtered.map((snippet) => (
            <Paper
              withBorder
              radius="lg"
              className="secondary-snippet"
              key={snippet.index}
            >
              <div className="secondary-snippet-top">
                <Group gap="sm">
                  <Text fw={600}>{snippet.trigger}</Text>
                  <Badge variant="light" color="gray">
                    {snippet.app || "All apps"}
                  </Badge>
                  {snippet.enabled === false && (
                    <Badge variant="light" color="gray">
                      Off
                    </Badge>
                  )}
                </Group>
                <Group gap={6} wrap="nowrap">
                  <Tooltip label="Edit snippet">
                    <ActionIcon
                      variant="subtle"
                      size={38}
                      aria-label={`Edit ${snippet.trigger}`}
                      onClick={() =>
                        setEditor({
                          trigger: snippet.trigger,
                          expansion: snippet.expansion,
                          app: snippet.app || "",
                          index: snippet.index,
                          expected_trigger: snippet.trigger,
                          expected_macro: structuredClone(
                            snippets[snippet.index],
                          ),
                        })
                      }
                    >
                      <PencilSimple size={19} />
                    </ActionIcon>
                  </Tooltip>
                  <Tooltip label="Delete snippet">
                    <ActionIcon
                      variant="subtle"
                      color="gray"
                      size={38}
                      aria-label={`Delete ${snippet.trigger}`}
                      onClick={() =>
                        setDeletion({
                          index: snippet.index,
                          name: snippet.trigger,
                          original: structuredClone(snippets[snippet.index]),
                        })
                      }
                    >
                      <Trash size={19} />
                    </ActionIcon>
                  </Tooltip>
                </Group>
              </div>
              <Text className="secondary-snippet-text" lineClamp={4}>
                {snippet.expansion}
              </Text>
              <Text c="dimmed" size="sm" mt="md">
                Say{" "}
                <span className="secondary-accent">
                  “bang {snippet.trigger}”
                </span>
              </Text>
            </Paper>
          ))}
        </Stack>
      ) : (
        <Empty
          icon={Scissors}
          title={
            search
              ? "No matching snippets"
              : "Skip the words you type again and again"
          }
          action={
            !search && (
              <Button
                variant="light"
                leftSection={<Plus size={17} />}
                onClick={add}
              >
                Create a snippet
              </Button>
            )
          }
        >
          {search
            ? "Try another name or a word from the snippet."
            : "Turn a short spoken cue into a complete reply, signature, or template."}
        </Empty>
      )}
      <Modal
        opened={Boolean(editor)}
        onClose={() => !busy && setEditor(null)}
        title={editor?.index == null ? "New snippet" : "Edit snippet"}
        centered
        size="lg"
        radius="lg"
      >
        <form onSubmit={save}>
          <Stack gap="lg">
            <TextInput
              label="Snippet name"
              description="One to four words. Say “bang” before this name when dictating."
              placeholder="For example, my signature"
              required
              autoFocus
              value={editor?.trigger || ""}
              onChange={(event) =>
                setEditor({ ...editor, trigger: event.currentTarget.value })
              }
            />
            <Textarea
              label="Text to insert"
              placeholder="Write the text you want to use…"
              required
              autosize
              minRows={6}
              maxRows={12}
              value={editor?.expansion || ""}
              onChange={(event) =>
                setEditor({ ...editor, expansion: event.currentTarget.value })
              }
            />
            <TextInput
              label="App"
              description="Leave empty to use this snippet in every app."
              placeholder="All apps, or an app such as discord.exe"
              value={editor?.app || ""}
              onChange={(event) =>
                setEditor({ ...editor, app: event.currentTarget.value })
              }
            />
            <Group justify="flex-end" mt="sm">
              <Button
                variant="default"
                onClick={() => setEditor(null)}
                disabled={busy}
              >
                Cancel
              </Button>
              <Button type="submit" loading={busy}>
                Save snippet
              </Button>
            </Group>
          </Stack>
        </form>
      </Modal>
      <DeleteDialog
        item={deletion}
        noun="snippet"
        onClose={() => !busy && setDeletion(null)}
        onConfirm={remove}
        busy={busy}
      />
    </div>
  );
}

function Metric({ label, value, description, icon: Icon }) {
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

export function InsightsPage({ state, act }) {
  const [refreshing, setRefreshing] = useState(false);
  const report = state.insights;
  const refresh = async () => {
    setRefreshing(true);
    try {
      await act({ type: "insights_refresh" });
    } catch {
      /* Root presents the error. */
    } finally {
      setRefreshing(false);
    }
  };
  useEffect(() => {
    act({ type: "insights_refresh" }).catch(() => {});
  }, [act]);
  const apps = report?.apps || [];
  const days = report?.days || [];
  const recentDays = days.slice(-7);
  const maxDayWords = Math.max(1, ...recentDays.map((day) => day.words));
  return (
    <div className="secondary-page">
      <Heading
        title="Insights"
        action={
          <Button
            variant="default"
            leftSection={<ArrowClockwise size={18} />}
            onClick={refresh}
            loading={refreshing}
          >
            Refresh
          </Button>
        }
      >
        See where your voice is saving you effort.
      </Heading>
      {!report ? (
        <Empty
          icon={ChartBar}
          title="Your activity, in perspective"
          action={
            state.history_loading ? (
              <Loader size="sm" />
            ) : (
              <Button variant="light" onClick={refresh} loading={refreshing}>
                Load insights
              </Button>
            )
          }
        >
          Insights are calculated from the dictations saved on this device.
        </Empty>
      ) : (
        <>
          <SimpleGrid cols={{ base: 1, sm: 3 }} spacing="lg">
            <Metric
              icon={Microphone}
              label="Words dictated"
              value={count(report.words)}
              description={`${count(report.sessions)} saved dictations`}
            />
            <Metric
              icon={Waveform}
              label="Speaking pace"
              value={
                report.words_per_minute == null
                  ? "—"
                  : `${Math.round(report.words_per_minute)} wpm`
              }
              description={
                report.words_per_minute == null
                  ? "Available after a measured dictation"
                  : "Based on recorded speaking time"
              }
            />
            <Metric
              icon={Sparkle}
              label="Helpful corrections"
              value={count(
                Number(report.dictionary_replacements || 0) +
                  Number(report.cleanup_edits || 0),
              )}
              description={`${count(report.dictionary_replacements)} vocabulary · ${count(report.cleanup_edits)} cleanup`}
            />
          </SimpleGrid>
          {report.sessions === 0 && (
            <Paper withBorder radius="lg" className="secondary-tip">
              <ThemeIcon size={42} variant="light" radius="md">
                <Microphone size={22} />
              </ThemeIcon>
              <div>
                <Text fw={600}>Start with a few words</Text>
                <Text c="dimmed" size="sm">
                  Your first saved dictation will start your activity history
                  here.
                </Text>
              </div>
            </Paper>
          )}
          <div className="secondary-insights-grid">
            <Paper withBorder radius="lg" className="secondary-insight-panel">
              <div className="secondary-panel-title">
                <h2>Where you dictate</h2>
                <Text c="dimmed" size="sm">
                  {count(apps.filter((app) => app.app).length)} apps
                </Text>
              </div>
              {apps.length ? (
                <Stack gap="xl" mt="lg">
                  {apps.map((app, index) => (
                    <div key={app.app || `unknown-${index}`}>
                      <Group justify="space-between" mb={8}>
                        <Text fw={500}>{app.app || "Other dictations"}</Text>
                        <Text c="dimmed" size="sm">
                          {count(app.words)} words
                        </Text>
                      </Group>
                      <Progress
                        size={9}
                        radius="xl"
                        value={
                          report.words
                            ? Math.min(100, (app.words / report.words) * 100)
                            : 0
                        }
                        aria-label={`${app.app || "Other dictations"} share of words`}
                      />
                    </div>
                  ))}
                </Stack>
              ) : (
                <Text c="dimmed" mt="lg">
                  App activity appears when you dictate into a text field.
                </Text>
              )}
            </Paper>
            <Paper withBorder radius="lg" className="secondary-insight-panel">
              <div className="secondary-panel-title">
                <h2>The last 7 days</h2>
                <Badge variant="light">
                  {count(report.current_streak)} day streak
                </Badge>
              </div>
              <Stack gap="md" mt="lg">
                {recentDays.map((day) => (
                  <div className="secondary-day" key={day.date}>
                    <Text c="dimmed" size="sm">
                      {new Date(`${day.date}T12:00:00`).toLocaleDateString(
                        undefined,
                        { weekday: "short" },
                      )}
                    </Text>
                    <Progress
                      size={9}
                      radius="xl"
                      value={(day.words / maxDayWords) * 100}
                      aria-label={`${day.date}: ${count(day.words)} words`}
                    />
                    <Text size="sm" ta="right">
                      {count(day.words)}
                    </Text>
                  </div>
                ))}
              </Stack>
              <Divider my="xl" />
              <Group justify="space-between">
                <Text c="dimmed" size="sm">
                  Longest streak
                </Text>
                <Text fw={500}>{count(report.longest_streak)} days</Text>
              </Group>
              <Group justify="space-between" mt="sm">
                <Text c="dimmed" size="sm">
                  Active days
                </Text>
                <Text fw={500}>{count(report.active_days)}</Text>
              </Group>
            </Paper>
          </div>
          <Text c="dimmed" size="sm" className="secondary-privacy">
            <ShieldCheck size={17} />
            Based on retained dictations on this device. Deleting a dictation
            also removes its activity.
            {report.dates_use_utc ? " Activity dates use UTC." : ""}
          </Text>
        </>
      )}
    </div>
  );
}

function SettingRow({ title, description, children }) {
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

function SetupStatus({ message, error, busy, progress }) {
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

export function SettingsPage({ state, act }) {
  const settings = state.settings || {};
  const notes = state.notes || {};
  const models = state.model_status || {};
  const discord = state.discord || {};
  const discordListening = Boolean(discord.listener_started);
  const usingCompanion = Boolean(
    discord.companion_selected ?? settings.discord_companion,
  );
  const discordStatus = {
    ready: discord.connected ? "Connected to Discord" : "Waiting for Discord",
    connecting: usingCompanion
      ? "Waiting for the companion"
      : "Connecting to Discord…",
    unavailable: "Discord is not available yet",
    disconnected: "Not connected",
  }[discord.status];
  const companion = discord.companion || {};
  const companionRuntime = {
    current: {
      label: "Latest companion running",
      color: "mint",
      description: "Discord is connected with the latest companion.",
    },
    restart_required: {
      label: "Restart Discord",
      color: "yellow",
      description:
        "The latest companion is installed. Quit Discord completely, including its tray icon, then reopen it to load the update. This will disconnect any active call.",
    },
    update_required: {
      label: "Update needed",
      color: "yellow",
      description:
        "Update the companion to install the current plugin and separate audio support.",
    },
    offline: {
      label: "Installed, not connected",
      color: "gray",
      description:
        "The latest companion is installed. Open Discord and connect to verify the version it is running.",
    },
    unverified: {
      label: "Not verified",
      color: "gray",
      description:
        "Check the installation and connect to Discord to verify the companion.",
    },
  }[companion.runtime_status || "unverified"];
  const companionNeedsUpdate =
    companion.runtime_status === "update_required" ||
    companion.plugin_status === "update_available";
  const updater = state.updater || {};
  const capturing =
    state.recording || state.call_recording || state.note_recording;
  const modelBusy =
    capturing ||
    state.busy ||
    state.loading ||
    state.summary_working ||
    state.filing_working ||
    models.cleanup_working ||
    models.speakers_working ||
    models.verifier_working ||
    models.context_progress != null ||
    notes.downloading ||
    state.downloading != null;
  const [pending, setPending] = useState("");
  const [shortcut, setShortcut] = useState(null);
  const [shortcutTarget, setShortcutTarget] = useState("hotkey");
  const [confirmation, setConfirmation] = useState(null);
  const [customModel, setCustomModel] = useState("");
  const [discordMode, setDiscordMode] = useState(
    settings.discord_companion ? "companion" : "standard",
  );
  useEffect(() => {
    setDiscordMode(settings.discord_companion ? "companion" : "standard");
  }, [settings.discord_companion]);
  const patch = async (name, value) => {
    setPending(name);
    try {
      await act({ type: "settings_patch", [name]: value });
    } catch {
      /* Root presents the error. */
    } finally {
      setPending("");
    }
  };
  const action = async (name, payload = {}) => {
    setPending(name);
    try {
      await act({ type: name, ...payload });
      return true;
    } catch {
      /* Root presents the error. */
      return false;
    } finally {
      setPending("");
    }
  };
  const toggle = (name, label) => (
    <Switch
      aria-label={label}
      size="md"
      checked={Boolean(settings[name])}
      disabled={Boolean(pending)}
      onChange={(event) => patch(name, event.currentTarget.checked)}
    />
  );
  const devices = (list, selected) =>
    [...new Set([...(list || []), ...(selected ? [selected] : [])])].map(
      (name) => ({ value: name, label: name }),
    );
  const hotkey = settings.hotkey;
  const languageNames = new Intl.DisplayNames(["en"], { type: "language" });
  const speechLanguages = [
    ...new Set([
      ...(models.speech_languages ||
        (state.preview ? ["en", "nl", "fr", "de", "es", "ja", "zh"] : [])),
      ...(settings.transcription_language
        ? [settings.transcription_language]
        : []),
    ]),
  ]
    .map((code) => {
      let label = code;
      try {
        label = languageNames.of(code) || code;
      } catch {
        /* Preserve an unfamiliar model code. */
      }
      return { value: code, label };
    })
    .sort((a, b) => a.label.localeCompare(b.label));
  const keybind = hotkey
    ? [
        hotkey.ctrl && "Ctrl",
        hotkey.alt && "Alt",
        hotkey.shift && "Shift",
        hotkey.win && "Win",
        hotkey.key,
      ].filter(Boolean)
    : [];
  const saveShortcut = async (event) => {
    event.preventDefault();
    setPending(shortcutTarget);
    try {
      await act({ type: "settings_patch", [shortcutTarget]: shortcut });
      setShortcut(null);
    } catch {
      /* Root presents the error. */
    } finally {
      setPending("");
    }
  };
  return (
    <div className="secondary-page secondary-settings">
      <Heading title="Settings">
        Your shortcuts, models, and connected apps, in one place.
      </Heading>
      {state.preview && (
        <Alert title="Browser preview" color="mint" mb="lg">
          Use the Articulate desktop app to install models, record audio, and
          connect Discord. The documents here are examples.
        </Alert>
      )}
      <Group gap="xs" mb="xl" aria-label="Settings sections">
        {[
          ["dictation", "Dictation"],
          ["audio", "Audio"],
          ["discord", "Discord"],
          ["models", "Models"],
          ["updates", "Updates"],
        ].map(([id, label]) => (
          <Button
            key={id}
            variant="light"
            size="sm"
            onClick={() =>
              document
                .getElementById(`settings-${id}`)
                ?.scrollIntoView({ block: "start", behavior: "smooth" })
            }
          >
            {label}
          </Button>
        ))}
      </Group>
      <section id="settings-dictation">
        <div className="secondary-section-title">
          <Keyboard size={22} />
          <h2>Dictation</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Keyboard shortcut"
            description="Use this shortcut in any app to start dictating."
          >
            <Group gap="md">
              <Group gap={5}>
                {keybind.map((key) => (
                  <kbd key={key}>{key}</kbd>
                ))}
              </Group>
              <Button
                variant="default"
                size="sm"
                disabled={Boolean(pending) || capturing}
                onClick={() => {
                  setShortcutTarget("hotkey");
                  setShortcut({
                    ...(hotkey || {
                      ctrl: true,
                      alt: true,
                      shift: false,
                      win: false,
                      key: "Space",
                    }),
                  });
                }}
              >
                Change
              </Button>
            </Group>
          </SettingRow>
          <SettingRow
            title="Shortcut behavior"
            description={
              settings.hotkey_mode === "Toggle"
                ? "Press once to start, then again to finish."
                : "Keep the shortcut held while you speak. Release to finish."
            }
          >
            <SegmentedControl
              aria-label="Shortcut behavior"
              data={[
                { value: "Hold", label: "Hold to talk" },
                { value: "Toggle", label: "Toggle" },
              ]}
              value={settings.hotkey_mode || "Hold"}
              onChange={(value) => patch("hotkey_mode", value)}
              disabled={Boolean(pending)}
            />
          </SettingRow>
          <SettingRow
            title="Sound feedback"
            description="Hear a cue when dictation, calls, or spoken notes start and stop."
          >
            {toggle("audio_feedback", "Sound feedback")}
          </SettingRow>
          <SettingRow
            title="Clean up speech"
            description="Remove fillers, repetitions, and clear spoken corrections automatically."
          >
            {toggle("clean_speech", "Clean up speech")}
          </SettingRow>
          <SettingRow
            title="Insert into the active field"
            description="Put your finished words where your cursor is."
          >
            {toggle("insert", "Insert into the active field")}
          </SettingRow>
          <SettingRow
            title="Show words as you speak"
            description="Update the active text field during dictation."
          >
            <Switch
              aria-label="Show words as you speak"
              size="md"
              checked={Boolean(settings.live_insert)}
              disabled={Boolean(pending) || !settings.insert}
              onChange={(event) =>
                patch("live_insert", event.currentTarget.checked)
              }
            />
          </SettingRow>
          <SettingRow
            title="Remember corrections"
            description="Learn short corrections you make after dictating."
          >
            {toggle("learn_corrections", "Remember corrections")}
          </SettingRow>
        </Paper>
      </section>
      <section>
        <div className="secondary-section-title">
          <BookOpen size={22} />
          <h2>Spoken notes</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Quick note shortcut"
            description="Press once to speak your thoughts into a note. Press again to finish. Your words stay in Notetaker."
          >
            <Group gap="md">
              <Group gap={5}>
                {settings.quick_note_hotkey ? (
                  [
                    settings.quick_note_hotkey.ctrl && "Ctrl",
                    settings.quick_note_hotkey.alt && "Alt",
                    settings.quick_note_hotkey.shift && "Shift",
                    settings.quick_note_hotkey.win && "Win",
                    settings.quick_note_hotkey.key,
                  ]
                    .filter(Boolean)
                    .map((key) => <kbd key={key}>{key}</kbd>)
                ) : (
                  <Text c="dimmed" size="sm">
                    Not configured
                  </Text>
                )}
              </Group>
              <Button
                variant="default"
                size="sm"
                disabled={Boolean(pending) || capturing}
                onClick={() => {
                  setShortcutTarget("quick_note_hotkey");
                  setShortcut({
                    ...(settings.quick_note_hotkey || {
                      ctrl: true,
                      alt: true,
                      shift: false,
                      win: false,
                      key: "N",
                    }),
                  });
                }}
              >
                Change
              </Button>
            </Group>
          </SettingRow>
          {settings.quick_note_hotkey_pending && (
            <Text size="sm" c="dimmed" px="lg" pb="md" role="status">
              Checking your quick note shortcut…
            </Text>
          )}
          {settings.quick_note_shortcut_error && (
            <Text size="sm" c="red" px="lg" pb="md" role="alert">
              {settings.quick_note_shortcut_error}
            </Text>
          )}
        </Paper>
      </section>
      <section id="settings-audio">
        <div className="secondary-section-title">
          <Microphone size={22} />
          <h2>Audio</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Transcription language"
            description="Set the recognition language for dictation and spoken notes. Calls have their own language preference and keep automatic detection."
          >
            <Select
              aria-label="Transcription language"
              searchable
              allowDeselect={false}
              data={[
                { value: "auto", label: "Auto-detect" },
                ...speechLanguages,
              ]}
              value={settings.transcription_language || "auto"}
              disabled={Boolean(pending) || capturing || state.loading}
              onChange={(value) => {
                if (value)
                  patch(
                    "transcription_language",
                    value === "auto" ? "" : value,
                  );
              }}
            />
          </SettingRow>
          <SettingRow
            title="Fast live transcription"
            description="Nemotron gives you fast live previews and checks short finished dictations. Qwen prepares the final text. Previews may change as you speak."
          >
            {models.verifier_installed ? (
              <Badge variant="light" leftSection={<Check size={14} />}>
                Installed
              </Badge>
            ) : (
              <Button
                variant="default"
                leftSection={<DownloadSimple size={17} />}
                loading={
                  models.verifier_working || pending === "verifier_download"
                }
                disabled={Boolean(pending) || modelBusy || capturing}
                onClick={() => action("verifier_download")}
              >
                Download · 751 MB
              </Button>
            )}
          </SettingRow>
          <SetupStatus
            busy={models.verifier_working}
            message={models.verifier_status}
            progress={models.verifier_progress}
          />
          <SettingRow
            title="Microphone"
            description="Choose the input for dictation and your voice in calls."
          >
            <Select
              aria-label="Microphone"
              placeholder="System default"
              searchable
              clearable
              data={devices(state.microphones, settings.microphone)}
              value={settings.microphone || null}
              onChange={(value) => patch("microphone", value ?? "")}
              disabled={
                Boolean(pending) ||
                state.recording ||
                state.call_recording ||
                state.note_recording
              }
              nothingFoundMessage="No matching microphones"
            />
          </SettingRow>
          <SettingRow
            title="Call audio output"
            description="Choose the device your call plays through."
          >
            <Select
              aria-label="Call audio output"
              placeholder="System default"
              searchable
              clearable
              data={devices(state.outputs, settings.output)}
              value={settings.output || null}
              onChange={(value) => patch("output", value ?? "")}
              disabled={Boolean(pending) || state.call_recording}
              nothingFoundMessage="No matching outputs"
            />
          </SettingRow>
        </Paper>
      </section>
      <section id="settings-discord">
        <div className="secondary-section-title">
          <Waveform size={22} />
          <h2>Discord calls</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Discord connection"
            description={
              discordStatus ||
              "Choose how Articulate connects to your Discord voice channel."
            }
          >
            <Stack gap="sm" align="flex-end">
              <Select
                label="Connection method"
                allowDeselect={false}
                aria-label="Discord connection method"
                value={discordMode}
                onChange={(value) => value && setDiscordMode(value)}
                disabled={
                  discordListening || companion.busy || Boolean(pending)
                }
                data={[
                  { value: "companion", label: "Vencord companion" },
                  { value: "standard", label: "Standard Discord" },
                ]}
              />
              <Group gap="sm">
                <Badge
                  variant="light"
                  color={discord.connected ? "mint" : "gray"}
                >
                  {discordStatus || "Not connected"}
                </Badge>
                <Button
                  variant="default"
                  loading={
                    pending === "discord_connect" ||
                    pending === "discord_disconnect"
                  }
                  disabled={
                    Boolean(pending) || discord.relaunching || capturing
                  }
                  onClick={() =>
                    discordListening
                      ? action("discord_disconnect")
                      : action("discord_connect", {
                          companion: discordMode === "companion",
                        })
                  }
                >
                  {discordListening ? "Disconnect" : "Connect"}
                </Button>
              </Group>
            </Stack>
          </SettingRow>
          {discord.error && <SetupStatus error={discord.error} />}
          {usingCompanion && (
            <>
              <SettingRow
                title="Companion pairing"
                description={
                  discord.pairing_state === "waiting"
                    ? "Use the latest companion, then restart Discord. Pairing happens automatically."
                    : "The companion pairs automatically with Articulate on this computer."
                }
              >
                <Badge
                  variant="light"
                  color={discord.pairing_state === "paired" ? "mint" : "gray"}
                >
                  {discord.pairing_state === "paired"
                    ? "Paired"
                    : discord.pairing_state === "waiting"
                      ? "Waiting for companion"
                      : "Automatic pairing"}
                </Badge>
              </SettingRow>
              <SettingRow
                title="Participant audio"
                description={
                  discord.audio_message ||
                  (discord.audio_ready
                    ? "Each participant's audio can be transcribed separately."
                    : discord.connected
                      ? "Speaker names are connected. Waiting for the companion's separate audio connection."
                      : "The separate audio connection becomes available after the companion connects.")
                }
              >
                <Stack gap="xs" align="flex-end">
                  <Badge
                    variant="light"
                    color={discord.audio_ready ? "mint" : "gray"}
                  >
                    {discord.audio_ready
                      ? "Separate audio ready"
                      : "Audio not ready"}
                  </Badge>
                  {discord.connected && (
                    <Text c="dimmed" size="sm">
                      {discord.in_voice
                        ? "Voice channel joined"
                        : "Waiting for a voice channel"}
                    </Text>
                  )}
                </Stack>
              </SettingRow>
            </>
          )}
          <SettingRow
            title="Vencord companion"
            description="Keep each participant's audio separate, with their Discord name attached to the transcript."
          >
            <Group gap="sm" wrap="wrap" justify="flex-end">
              <Button
                variant="default"
                onClick={() => action("companion_detect")}
                loading={pending === "companion_detect"}
                disabled={companion.busy || Boolean(pending)}
              >
                Check installation
              </Button>
              {(companionNeedsUpdate ||
                companion.plugin_status !== "current") && (
                <Button
                  onClick={() => action("companion_install")}
                  loading={companion.busy && pending !== "companion_detect"}
                  disabled={companion.busy || capturing || Boolean(pending)}
                >
                  {companionNeedsUpdate
                    ? "Update companion"
                    : "Install companion"}
                </Button>
              )}
              {(companion.installed ||
                companion.source_found ||
                companion.plugin_status === "current") && (
                <Button
                  variant="subtle"
                  onClick={() => action("companion_install")}
                  disabled={companion.busy || capturing || Boolean(pending)}
                >
                  Repair companion
                </Button>
              )}
              <Badge variant="light" color={companionRuntime?.color || "gray"}>
                {companionRuntime?.label || "Not verified"}
              </Badge>
              {companion.installer_ready && (
                <Button
                  variant="default"
                  onClick={() => action("companion_open_installer")}
                  disabled={companion.busy || capturing || Boolean(pending)}
                >
                  Open Discord installer
                </Button>
              )}
            </Group>
          </SettingRow>
          <SetupStatus message={companionRuntime?.description} />
          <SetupStatus
            busy={companion.busy}
            message={companion.busy ? companion.status : undefined}
            error={companion.error}
          />
          <SettingRow
            title="Keep the companion up to date"
            description="Update the installed companion when a newer version is available."
          >
            {toggle(
              "vencord_auto_update",
              "Update the Vencord companion automatically",
            )}
          </SettingRow>
          {discordMode === "standard" && !settings.discord_companion && (
            <SettingRow
              title="Standard Discord setup"
              description="Restart Discord with local debugging enabled so Articulate can read speaker activity. This briefly disconnects your call."
            >
              <Button
                variant="default"
                loading={discord.relaunching}
                disabled={capturing || Boolean(pending) || discord.relaunching}
                onClick={() => setConfirmation("discord_relaunch")}
              >
                Relaunch Discord
              </Button>
            </SettingRow>
          )}
          <SettingRow
            title="Connect automatically"
            description="Connect to Discord when Articulate is open."
          >
            {toggle("discord_auto_connect", "Connect to Discord automatically")}
          </SettingRow>
          <SettingRow
            title="Transcribe calls automatically"
            description="Start when you join a voice channel. Finish and save when you leave."
          >
            {toggle(
              "discord_auto_transcribe",
              "Transcribe Discord calls automatically",
            )}
          </SettingRow>
          <SetupStatus message={discord.automatic_status} />
          {discord.automatic_can_retry && (
            <Group justify="flex-end" pb="lg">
              <Button
                variant="light"
                loading={pending === "discord_retry"}
                disabled={capturing || Boolean(pending) || discord.relaunching}
                onClick={() => action("discord_retry")}
              >
                Retry automatic capture
              </Button>
            </Group>
          )}
        </Paper>
      </section>
      <section id="settings-models">
        <div className="secondary-section-title">
          <Sparkle size={22} />
          <h2>Models</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title={
              state.ready
                ? "Ready to listen"
                : state.loading
                  ? "Preparing your speech model"
                  : "Set up local transcription"
            }
            description={
              state.ready
                ? "Your speech is transcribed on this computer."
                : state.status ||
                  "Download the speech model once to get started."
            }
          >
            <Group wrap="wrap" gap="sm">
              {state.ready ? (
                <Badge
                  leftSection={<Check size={14} />}
                  size="lg"
                  variant="light"
                >
                  Ready
                </Badge>
              ) : (
                <>
                  <Button
                    variant="default"
                    leftSection={<DownloadSimple size={17} />}
                    onClick={() => action("model_download")}
                    loading={pending === "model_download"}
                    disabled={
                      state.loading ||
                      state.busy ||
                      state.downloading != null ||
                      state.recording ||
                      state.call_recording ||
                      state.note_recording ||
                      Boolean(pending)
                    }
                  >
                    Download model
                  </Button>
                  <Button
                    leftSection={<ArrowRight size={17} />}
                    onClick={() => action("model_load")}
                    loading={state.loading || pending === "model_load"}
                    disabled={
                      state.busy ||
                      state.downloading != null ||
                      state.recording ||
                      state.call_recording ||
                      state.note_recording ||
                      Boolean(pending)
                    }
                  >
                    Load model
                  </Button>
                </>
              )}
            </Group>
          </SettingRow>
        </Paper>
      </section>
      <Paper withBorder radius="lg" className="secondary-settings-card" mt="sm">
        <SettingRow
          title="Speech processing"
          description={
            models.backend
              ? `Currently using ${models.backend}.`
              : "Use your graphics card when available, or choose CPU processing."
          }
        >
          <Switch
            label="Use CPU"
            checked={Boolean(settings.cpu)}
            disabled={modelBusy || Boolean(pending)}
            onChange={(event) => patch("cpu", event.currentTarget.checked)}
          />
        </SettingRow>
        <details style={{ paddingBottom: 22 }}>
          <summary>Advanced speech model</summary>
          <Text size="sm" c="dimmed" mt="sm">
            {models.model_name
              ? `Current model: ${models.model_name}`
              : "Use your own compatible GGUF speech model."}
          </Text>
          <TextInput
            mt="md"
            label="Custom model file"
            placeholder="Full path to a compatible .gguf model"
            value={customModel}
            onChange={(event) => setCustomModel(event.currentTarget.value)}
            disabled={modelBusy || Boolean(pending)}
          />
          <Group justify="flex-end" mt="sm">
            <Button
              variant="default"
              disabled={!customModel.trim() || modelBusy || Boolean(pending)}
              loading={pending === "model_path"}
              onClick={() => patch("model_path", customModel.trim())}
            >
              Use this model
            </Button>
          </Group>
        </details>
      </Paper>
      {state.downloading != null && (
        <Paper withBorder radius="lg" className="secondary-download">
          <Group justify="space-between" mb="sm">
            <Text fw={500}>Downloading speech model</Text>
            <Text c="dimmed" size="sm">
              {Math.round(state.downloading * 100)}%
            </Text>
          </Group>
          <Progress
            value={state.downloading * 100}
            size="sm"
            radius="xl"
            aria-label="Speech model download progress"
          />
        </Paper>
      )}

      <section>
        <div className="secondary-section-title">
          <BookOpen size={22} />
          <h2>Notes model</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title={
              notes.ready
                ? "Ready for your conversations"
                : notes.downloading
                  ? "Downloading your notes model"
                  : "Turn conversations into useful notes"
            }
            description="Generate notes and descriptive call titles on this computer. Download once, then use it offline."
          >
            <Group gap="sm" wrap="wrap">
              {notes.ready ? (
                <Badge
                  leftSection={<Check size={14} />}
                  size="lg"
                  variant="light"
                >
                  Ready
                </Badge>
              ) : (
                <Button
                  leftSection={<DownloadSimple size={17} />}
                  onClick={() => action("summary_download")}
                  loading={notes.downloading || pending === "summary_download"}
                  disabled={Boolean(pending) || modelBusy}
                >
                  Download notes model
                </Button>
              )}
              {notes.downloading && (
                <Button
                  variant="subtle"
                  onClick={() => action("summary_download_cancel")}
                  disabled={Boolean(pending)}
                >
                  Cancel download
                </Button>
              )}
            </Group>
          </SettingRow>
          {notes.ready && (
            <SettingRow
              title="Faster local notes"
              description={
                notes.acceleration_installed
                  ? "Acceleration tools are installed. A compatible graphics card is used when available; otherwise notes use the CPU."
                  : "Use a compatible graphics card to generate notes faster. Download the optional acceleration tools once (32 MB)."
              }
            >
              {notes.acceleration_installed ? (
                <Badge variant="light">Tools installed</Badge>
              ) : (
                <Button
                  variant="default"
                  onClick={() => action("summary_download")}
                  loading={notes.downloading || pending === "summary_download"}
                  disabled={
                    Boolean(pending) ||
                    notes.working ||
                    state.recording ||
                    state.call_recording ||
                    state.note_recording ||
                    modelBusy
                  }
                >
                  Download acceleration
                </Button>
              )}
            </SettingRow>
          )}
          {(notes.download_status || notes.working) && (
            <div className="secondary-model-status" role="status">
              <Group align="flex-start" gap="sm">
                {(notes.downloading || notes.working) && <Loader size="xs" />}
                <Text c="dimmed" size="sm">
                  {notes.working
                    ? notes.status || "Updating your notes…"
                    : notes.download_status}
                </Text>
              </Group>
              {notes.working && notes.progress != null && (
                <Progress
                  value={Math.max(0, Math.min(100, notes.progress * 100))}
                  size="sm"
                  radius="xl"
                  mt="md"
                  aria-label="Notes progress"
                />
              )}
            </div>
          )}
        </Paper>
      </section>
      <section>
        <div className="secondary-section-title">
          <Sparkle size={22} />
          <h2>Text cleanup</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Local writing editor"
            description="Download the editor for advanced text cleanup. Automatic filler and repetition removal works without this download."
          >
            {models.cleanup_installed ? (
              <Badge variant="light" leftSection={<Check size={14} />}>
                Installed
              </Badge>
            ) : (
              <Button
                leftSection={<DownloadSimple size={17} />}
                loading={
                  models.cleanup_working || pending === "cleanup_download"
                }
                disabled={Boolean(pending) || modelBusy}
                onClick={() => action("cleanup_download")}
              >
                Download cleanup model
              </Button>
            )}
          </SettingRow>
          <SetupStatus
            busy={models.cleanup_working}
            message={models.cleanup_status}
            progress={models.cleanup_progress}
          />
          {models.cleanup_working && (
            <Button
              variant="subtle"
              mb="md"
              onClick={() => action("cleanup_download_cancel")}
              disabled={Boolean(pending)}
            >
              Cancel download
            </Button>
          )}
        </Paper>
      </section>
      <section>
        <div className="secondary-section-title">
          <Waveform size={22} />
          <h2>Audio context</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="SenseVoice audio cues"
            description="Show laughter, crying and possible vocal tone beside the speaker. Runs locally in the background."
          >
            <Button
              variant="default"
              leftSection={<DownloadSimple size={17} />}
              loading={
                models.context_progress != null ||
                pending === "context_download"
              }
              disabled={
                Boolean(pending) ||
                modelBusy ||
                state.preview ||
                models.context_supported === false
              }
              onClick={() => action("context_download")}
            >
              {models.context_installed
                ? "Reinstall audio context"
                : "Download · 259 MB"}
            </Button>
          </SettingRow>
          <SetupStatus
            busy={models.context_progress != null}
            message={models.context_status}
            progress={models.context_progress}
          />
          <SettingRow
            title="Include sound and tone cues"
            description="New recordings are checked for sound events and non-neutral vocal tone. Some passages have no cue; spoken words stay unchanged."
          >
            <Switch
              aria-label="Include sound and tone cues"
              size="md"
              checked={Boolean(settings.audio_context)}
              disabled={
                Boolean(pending) || modelBusy || !models.context_installed
              }
              onChange={(event) =>
                patch("audio_context", event.currentTarget.checked)
              }
            />
          </SettingRow>
        </Paper>
      </section>
      <section>
        <div className="secondary-section-title">
          <Waveform size={22} />
          <h2>Speaker recognition</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title="Recognize speakers in mixed audio"
            description="Useful for calls without separate participant audio. The Vencord companion already identifies each participant directly."
          >
            {models.speakers_installed ? (
              <Badge variant="light" leftSection={<Check size={14} />}>
                Installed
              </Badge>
            ) : (
              <Button
                variant="default"
                leftSection={<DownloadSimple size={17} />}
                loading={
                  models.speakers_working || pending === "speakers_download"
                }
                disabled={Boolean(pending) || modelBusy}
                onClick={() => action("speakers_download")}
              >
                Download speaker model
              </Button>
            )}
          </SettingRow>
          <SetupStatus
            busy={models.speakers_working}
            message={models.speakers_status}
            progress={models.speakers_progress}
          />
        </Paper>
      </section>
      <section id="settings-updates">
        <div className="secondary-section-title">
          <ArrowClockwise size={22} />
          <h2>App updates</h2>
        </div>
        <Paper withBorder radius="lg" className="secondary-settings-card">
          <SettingRow
            title={
              updater.state === "available" || updater.state === "ready"
                ? `Articulate ${updater.version || "update"} is available`
                : "Keep Articulate up to date"
            }
            description={
              updater.state === "latest"
                ? "You're running the latest available release."
                : "Check for new features and fixes. Your notes and downloaded models stay on this computer."
            }
          >
            <Group gap="sm" wrap="wrap" justify="flex-end">
              <Button
                variant="default"
                onClick={() => action("update_check")}
                loading={
                  updater.state === "checking" || pending === "update_check"
                }
                disabled={
                  Boolean(pending) ||
                  ["checking", "downloading"].includes(updater.state)
                }
              >
                Check for updates
              </Button>
              {updater.state === "available" && (
                <Button
                  leftSection={<DownloadSimple size={17} />}
                  onClick={() => action("update_download")}
                  disabled={Boolean(pending) || capturing}
                >
                  {updater.size
                    ? `Download · ${Math.ceil(updater.size / 1000000)} MB`
                    : "Download update"}
                </Button>
              )}
              {updater.state === "ready" && (
                <Button
                  onClick={() => setConfirmation("update_install")}
                  disabled={
                    Boolean(pending) || capturing || state.saving || modelBusy
                  }
                >
                  Install update
                </Button>
              )}
            </Group>
          </SettingRow>
          <SetupStatus
            busy={updater.state === "downloading"}
            message={
              updater.state === "downloading"
                ? "Downloading the update…"
                : undefined
            }
            progress={updater.progress}
            error={updater.error}
          />
        </Paper>
      </section>
      <Text c="dimmed" size="sm" className="secondary-privacy">
        <ShieldCheck size={17} />
        Your transcripts, vocabulary, and preferences stay on this device.
      </Text>
      <Modal
        opened={Boolean(confirmation)}
        onClose={() => !pending && setConfirmation(null)}
        title={
          confirmation === "discord_relaunch"
            ? "Relaunch Discord?"
            : "Install the update?"
        }
        centered
        size="md"
      >
        <Text c="dimmed">
          {confirmation === "discord_relaunch"
            ? "Discord will restart with local debugging enabled. Any current voice call will disconnect. Restart Discord normally later to turn debugging off."
            : "Your notes will be saved before the updater starts. Articulate will close to finish installing the update."}
        </Text>
        <Group justify="flex-end" mt="xl">
          <Button
            variant="default"
            disabled={Boolean(pending)}
            onClick={() => setConfirmation(null)}
          >
            Cancel
          </Button>
          <Button
            loading={Boolean(pending)}
            onClick={async () => {
              if (await action(confirmation)) setConfirmation(null);
            }}
          >
            {confirmation === "discord_relaunch"
              ? "Relaunch Discord"
              : "Install update"}
          </Button>
        </Group>
      </Modal>
      <Modal
        opened={Boolean(shortcut)}
        onClose={() => !pending && setShortcut(null)}
        title={
          shortcutTarget === "quick_note_hotkey"
            ? "Change quick note shortcut"
            : "Change keyboard shortcut"
        }
        centered
        radius="lg"
        size="md"
      >
        <form onSubmit={saveShortcut}>
          <Stack gap="lg">
            <Text c="dimmed">
              {shortcutTarget === "quick_note_hotkey"
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
                  checked={Boolean(shortcut?.[field])}
                  onChange={(event) =>
                    setShortcut({
                      ...shortcut,
                      [field]: event.currentTarget.checked,
                    })
                  }
                />
              ))}
            </Group>
            <Select
              label="Key"
              data={shortcutKeys}
              searchable
              allowDeselect={false}
              value={shortcut?.key || "Space"}
              onChange={(value) =>
                value && setShortcut({ ...shortcut, key: value })
              }
            />
            <Text size="sm" c="dimmed">
              Include Ctrl, Alt, or Win to keep ordinary typing available.
            </Text>
            <Group justify="flex-end">
              <Button
                variant="default"
                disabled={Boolean(pending)}
                onClick={() => setShortcut(null)}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                loading={pending === shortcutTarget}
                disabled={!(shortcut?.ctrl || shortcut?.alt || shortcut?.win)}
              >
                Save shortcut
              </Button>
            </Group>
          </Stack>
        </form>
      </Modal>
    </div>
  );
}
