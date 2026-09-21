import { useCallback, useEffect, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Avatar,
  Badge,
  Button,
  Divider,
  Group,
  Kbd,
  Menu,
  Modal,
  SegmentedControl,
  Select,
  Skeleton,
  Tabs,
  Textarea,
  TextInput,
  Tooltip,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import {
  ArrowLeft,
  ArrowUpRight,
  BookOpen,
  ChartBar,
  Check,
  Clock,
  Copy,
  DotsThree,
  DownloadSimple,
  GearSix,
  Headphones,
  Lightning,
  LockSimple,
  MagnifyingGlass,
  Microphone,
  Notebook,
  Plus,
  Record,
  Sparkle,
  Stop,
  TextAlignLeft,
  Trash,
  Users,
  X,
} from "@phosphor-icons/react";
import {
  action,
  snapshot,
  onCloseRequested,
  closeDesktop,
  sourceMoveAction,
} from "./bridge";
import { NavHighlight } from "./motion.jsx";
import { createCaptureFeedbackTracker } from "./capture-feedback.js";
import { TranscriptEditor } from "./TranscriptEditor.jsx";
import {
  VocabularyPage,
  SnippetsPage,
  InsightsPage,
  SettingsPage,
} from "./SecondaryPages";
const navigation = [
  ["dictation", "Dictation", Microphone],
  ["notetaker", "Notetaker", Notebook],
  ["insights", "Insights", ChartBar],
  ["vocabulary", "Vocabulary", BookOpen],
  ["snippets", "Snippets", Lightning],
];
const date = (ms) =>
  new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(ms);
const duration = (ms) =>
  ms ? `${Math.max(1, Math.round(ms / 60000))} min` : "";
const time = (ms) =>
  `${Math.floor(ms / 60000)
    .toString()
    .padStart(2, "0")}:${Math.floor((ms / 1000) % 60)
    .toString()
    .padStart(2, "0")}`;
const iconFor = (kind) =>
  kind === "call" ? Headphones : kind === "note" ? Notebook : Microphone;
const words = (text) => text.trim().split(/\s+/).filter(Boolean).length;
const shortcutLabel = (hotkey) =>
  [
    hotkey?.ctrl && "Ctrl",
    hotkey?.alt && "Alt",
    hotkey?.shift && "Shift",
    hotkey?.win && "Win",
    hotkey?.key,
  ]
    .filter(Boolean)
    .join(" + ");
export function App() {
  const [state, setState] = useState(null),
    [page, setPage] = useState("notetaker"),
    [workspace, setWorkspace] = useState("library"),
    [error, setError] = useState(""),
    [edits, setEdits] = useState({}),
    [savingNow, setSavingNow] = useState(false),
    [documentLocked, setDocumentLocked] = useState(false);
  const pending = useRef(new Map()),
    timer = useRef(null),
    saving = useRef(null),
    refreshSequence = useRef(0),
    appliedSequence = useRef(0),
    transition = useRef(false),
    openedCapture = useRef(null),
    captureFeedback = useRef(createCaptureFeedbackTracker()),
    openCapture = useRef(null),
    contentScroll = useRef(null);
  useEffect(() => {
    contentScroll.current?.scrollTo({ top: 0 });
  }, [page, workspace, state?.selected?.id]);
  const refresh = useCallback(async () => {
    const request = ++refreshSequence.current;
    const next = await snapshot();
    if (request < appliedSequence.current) return;
    appliedSequence.current = request;
    setState(next);
    setError("");
    setEdits((old) => {
      const clean = { ...old };
      for (const s of [
        next.selected,
        next.call_session,
        next.note_session,
      ].filter(Boolean)) {
        if (
          clean[s.id] &&
          !pending.current.has(s.id) &&
          Object.entries(clean[s.id]).every(
            ([k, v]) => s[k] === (k === "title" ? v.trim() : v),
          )
        )
          delete clean[s.id];
      }
      return Object.keys(clean).length === Object.keys(old).length
        ? old
        : clean;
    });
  }, []);
  useEffect(() => {
    let active = true,
      id;
    const poll = async () => {
      try {
        if (active) await refresh();
      } catch (e) {
        if (active) setError(String(e));
      } finally {
        if (active) id = setTimeout(poll, 500);
      }
    };
    void poll();
    return () => {
      active = false;
      clearTimeout(id);
    };
  }, [refresh]);
  const report = useCallback(
    (e) =>
      notifications.show({
        title: "Could not complete that",
        message: String(e),
        color: "red",
      }),
    [],
  );
  const flush = useCallback(async () => {
    clearTimeout(timer.current);
    if (saving.current) return saving.current;
    if (!pending.current.size) return;
    setSavingNow(true);
    const work = (async () => {
      while (pending.current.size) {
        for (const [id, patch] of [...pending.current.entries()]) {
          await action({ type: "session_patch", id, ...patch });
          if (pending.current.get(id) === patch) pending.current.delete(id);
        }
      }
      await refresh();
    })();
    saving.current = work;
    try {
      await work;
    } finally {
      if (saving.current === work) saving.current = null;
      setSavingNow(false);
    }
  }, [refresh]);
  const act = useCallback(
    async (command) => {
      const changesDocument = [
        "history_open",
        "history_close",
        "note_new",
        "note_capture_start",
        "note_capture_stop",
        "session_delete",
        "dictation_edit",
        "dictation_start",
        "call_start",
        "call_stop",
        "notes_generate",
        "source_move",
        "source_file",
        "update_install",
      ].includes(command.type);
      if (changesDocument && transition.current) {
        throw new Error(
          "Please wait for the current document action to finish.",
        );
      }
      if (changesDocument) {
        transition.current = true;
        setDocumentLocked(true);
      }
      try {
        if (
          [
            "history_open",
            "history_close",
            "note_new",
            "note_capture_start",
            "note_capture_stop",
            "session_delete",
            "dictation_edit",
            "dictation_start",
            "call_start",
            "call_stop",
            "notes_generate",
            "source_move",
            "source_file",
            "update_install",
          ].includes(command.type)
        )
          await flush();
        await action(command);
        await refresh();
        if (command.type === "copy")
          notifications.show({
            message: "Copied to clipboard",
            icon: <Check size={18} />,
            autoClose: 2200,
          });
      } catch (e) {
        report(e);
        throw e;
      } finally {
        if (changesDocument) {
          transition.current = false;
          setDocumentLocked(false);
        }
      }
    },
    [flush, refresh, report],
  );
  function edit(id, patch) {
    if (transition.current) return;
    pending.current.set(id, { ...(pending.current.get(id) || {}), ...patch });
    setEdits((old) => ({ ...old, [id]: { ...(old[id] || {}), ...patch } }));
    clearTimeout(timer.current);
    timer.current = setTimeout(() => void flush().catch(report), 650);
  }
  useEffect(() => {
    const before = (e) => {
      if (pending.current.size) {
        e.preventDefault();
        e.returnValue = "";
      }
    };
    const key = (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "s") {
        e.preventDefault();
        void flush().catch(report);
      }
    };
    window.addEventListener("beforeunload", before);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("beforeunload", before);
      window.removeEventListener("keydown", key);
    };
  }, [flush, report]);
  useEffect(() => {
    let stopped = false,
      unlisten;
    onCloseRequested(async () => {
      if (transition.current) return;
      transition.current = true;
      setDocumentLocked(true);
      try {
        do {
          await flush();
        } while (pending.current.size);
        await closeDesktop();
      } catch (e) {
        report(e);
      } finally {
        transition.current = false;
        setDocumentLocked(false);
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
  }, [flush, report]);
  async function navigate(next) {
    try {
      await flush();
      setPage(next);
    } catch (e) {
      report(e);
    }
  }
  async function open(id) {
    try {
      await act({ type: "history_open", id });
      setWorkspace(
        state?.note_recording && state.note_session?.id === id
          ? "live-note"
          : state?.call_recording && state.call_session?.id === id
            ? "live"
            : "saved",
      );
      setPage("notetaker");
    } catch {}
  }
  async function newNote() {
    try {
      await act({ type: "note_new" });
      setWorkspace("saved");
      setPage("notetaker");
    } catch {}
  }
  useEffect(() => {
    openCapture.current = open;
  });
  useEffect(() => {
    const feedback = captureFeedback.current(state?.capture_feedback);
    if (!feedback) return;
    notifications.show({
      id: `capture-feedback-${feedback.sequence}`,
      title: feedback.title,
      message: (
        <div>
          <span>{feedback.message}</span>
          {feedback.sessionId && (
            <Button
              variant="subtle"
              size="compact-sm"
              mt="sm"
              onClick={() => void openCapture.current?.(feedback.sessionId)}
            >
              Open in Notetaker
            </Button>
          )}
        </div>
      ),
      icon:
        feedback.event === "started" ? (
          <Record size={18} weight="fill" />
        ) : feedback.event === "failed" ? (
          <X size={18} />
        ) : (
          <Stop size={18} weight="fill" />
        ),
      color: feedback.event === "failed" ? "red" : "mint",
      autoClose: feedback.event === "failed" ? 10000 : 5500,
      role: "status",
      "aria-live": "polite",
      "aria-atomic": true,
      closeButtonProps: { "aria-label": "Dismiss recording confirmation" },
    });
  }, [state?.capture_feedback]);
  useEffect(() => {
    const id = state?.note_recording && state.note_session?.id;
    if (!id || openedCapture.current === id) return;
    openedCapture.current = id;
    void flush()
      .then(() => {
        setPage("notetaker");
        setWorkspace("live-note");
      })
      .catch(report);
  }, [state?.note_recording, state?.note_session?.id, flush, report]);
  const selected =
      workspace === "live-note"
        ? state?.note_session || state?.selected
        : workspace === "live"
          ? state?.call_session
          : state?.selected,
    document = selected ? { ...selected, ...edits[selected.id] } : null;
  const saveState = error
    ? "Connection unavailable"
    : state?.history_error
      ? "Save failed"
      : savingNow || state?.saving || Object.keys(edits).length
        ? "Saving…"
        : "Saved on this device";
  const hotkey = state?.settings?.hotkey;
  const binding = [
    hotkey?.ctrl ? "Ctrl" : null,
    hotkey?.alt ? "Alt" : null,
    hotkey?.shift ? "Shift" : null,
    hotkey?.win ? "Win" : null,
    hotkey?.key || "Space",
  ]
    .filter(Boolean)
    .join(" + ");
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <a
          className="brand"
          href="#notetaker"
          onClick={(e) => {
            e.preventDefault();
            void navigate("notetaker");
          }}
        >
          <img src="/assets/articulate-mark.svg" alt="" />
          <span>Articulate</span>
        </a>
        <div className="workspace-label">YOUR WORKSPACE</div>
        <nav aria-label="Workspace">
          {navigation.map(([id, label, Icon]) => (
            <button
              key={id}
              className={`nav-item ${page === id ? "active" : ""}`}
              aria-current={page === id ? "page" : undefined}
              onClick={() => void navigate(id)}
            >
              {page === id && <NavHighlight page={page} />}
              <Icon size={21} weight={page === id ? "duotone" : "regular"} />
              <span>{label}</span>
              {page === id && <span className="nav-dot" />}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="shortcut-tip">
            <Kbd>{binding}</Kbd>
            <p>
              {state?.settings?.hotkey_mode === "Toggle"
                ? "Press to start dictating"
                : "Hold to speak, release to finish"}
            </p>
          </div>
          <button
            className={`nav-item ${page === "settings" ? "active" : ""}`}
            onClick={() => void navigate("settings")}
            aria-current={page === "settings" ? "page" : undefined}
          >
            {page === "settings" && <NavHighlight page={page} />}
            <GearSix size={21} />
            <span>Settings</span>
          </button>
          <div className="privacy">
            <LockSimple size={15} />
            <span>On this device</span>
            <span className="privacy-dot" />
          </div>
        </div>
      </aside>
      <main className="main-shell">
        <header className="app-topbar">
          <span>
            {page === "notetaker" && workspace !== "library"
              ? workspace === "live-note"
                ? "Notetaker / Spoken note"
                : "Notetaker / Your notes"
              : navigation.find((n) => n[0] === page)?.[1] || "Settings"}
          </span>
          <div className="topbar-right">
            <LockSimple size={14} />
            <span>Your words stay yours</span>
            <Avatar size={29} radius="xl" color="mint">
              Y
            </Avatar>
          </div>
        </header>
        {state?.call_recording &&
          !(page === "notetaker" && workspace === "live") && (
            <button
              className="recording-banner"
              onClick={() => {
                setPage("notetaker");
                setWorkspace("live");
              }}
            >
              <Record size={17} weight="fill" />
              <span>Conversation is recording</span>
              <span>
                Return to call <ArrowUpRight size={15} />
              </span>
            </button>
          )}
        {state?.note_recording &&
          !(page === "notetaker" && workspace === "live-note") && (
            <button
              className="recording-banner"
              onClick={() => {
                setPage("notetaker");
                setWorkspace("live-note");
              }}
            >
              <Record size={17} weight="fill" />
              <span>Listening to your thoughts</span>
              <span>
                Return to note <ArrowUpRight size={15} />
              </span>
            </button>
          )}
        <div
          ref={contentScroll}
          className={`main-content ${page === "notetaker" && workspace !== "library" ? "reader-mode" : ""}`}
        >
          {error && (
            <Alert color="red" title="Connection unavailable" mb="md">
              {error}
            </Alert>
          )}
          {state?.history_error && (
            <Alert
              color="red"
              title="Your latest changes need attention"
              mb="md"
            >
              {state.history_error}
              <Button
                variant="subtle"
                size="xs"
                onClick={() =>
                  void act({ type: "history_retry" }).catch(() => {})
                }
              >
                Retry saving
              </Button>
            </Alert>
          )}
          {!state?.settings || !Array.isArray(state.history) ? (
            <div className="page-content">
              <Skeleton height={36} width={280} mb="xl" />
              {[0, 1, 2].map((i) => (
                <Skeleton key={i} height={120} mb="md" />
              ))}
            </div>
          ) : page === "notetaker" ? (
            workspace === "library" ? (
              <Library
                state={state}
                open={open}
                newNote={newNote}
                speak={() =>
                  void act({ type: "note_capture_start" })
                    .then(() => setWorkspace("live-note"))
                    .catch(() => {})
                }
                start={() =>
                  void act({ type: "call_start" })
                    .then(() => setWorkspace("live"))
                    .catch(() => {})
                }
              />
            ) : (
              <Reader
                session={document}
                rows={
                  workspace === "live-note"
                    ? state.note_rows || document?.rows
                    : workspace === "live"
                      ? state.call_rows
                      : document?.rows
                }
                live={
                  workspace === "live-note"
                    ? state.note_recording
                    : workspace === "live" && state.call_recording
                }
                spokenNote={
                  workspace === "live-note" ||
                  (document?.kind === "note" && Boolean(document?.text))
                }
                status={
                  workspace === "live-note"
                    ? state.note_status
                    : state.call_status
                }
                seconds={
                  workspace === "live-note"
                    ? (state.note_seconds ?? state.seconds)
                    : state.seconds
                }
                saveState={saveState}
                onEdit={(patch) => document && edit(document.id, patch)}
                locked={documentLocked || state.source_move_working}
                back={() =>
                  void flush()
                    .then(() => setWorkspace("library"))
                    .catch(report)
                }
                act={act}
                topics={state.history.filter(
                  (item) => item.can_receive_sources ?? item.is_collection,
                )}
                openSession={open}
                moveBlocked={
                  state.recording ||
                  state.call_recording ||
                  state.note_recording ||
                  state.busy ||
                  state.summary_working ||
                  state.filing_working
                }
                filingStatus={state.filing_status}
                filingWorking={state.filing_working}
                notesBusy={state.summary_working || state.filing_working}
                onDeleted={() => setWorkspace("library")}
                notesState={
                  workspace === "live-note"
                    ? state.note_notes
                    : workspace === "live"
                      ? state.call_notes
                      : state.selected_notes
                }
              />
            )
          ) : page === "dictation" ? (
            <Dictation
              state={state}
              act={act}
              open={open}
              settings={() => void navigate("settings")}
              binding={binding}
            />
          ) : page === "vocabulary" ? (
            <VocabularyPage state={state} act={act} />
          ) : page === "snippets" ? (
            <SnippetsPage state={state} act={act} />
          ) : page === "insights" ? (
            <InsightsPage state={state} act={act} />
          ) : (
            <SettingsPage state={state} act={act} />
          )}
        </div>
      </main>
    </div>
  );
}
function Library({ state, open, newNote, start, speak }) {
  const [query, setQuery] = useState(""),
    [kind, setKind] = useState("all"),
    [preview, setPreview] = useState(null);
  const filtered = state.history.filter(
    (s) =>
      (kind === "all" || s.kind === kind) &&
      (query || kind === "dictation" || !s.topic_id) &&
      `${s.title} ${s.preview}`.toLowerCase().includes(query.toLowerCase()),
  );
  const selected = filtered.find((s) => s.id === preview) || filtered[0];
  return (
    <div className="page-content library-page">
      <div className="page-header">
        <div>
          <div className="eyebrow">SPACE TO THINK</div>
          <h1 className="page-heading">Notetaker</h1>
          <p className="page-subtitle">
            Every conversation. A little more clarity.
          </p>
        </div>
        <Group gap={10}>
          <Button
            variant="default"
            leftSection={<Plus size={17} />}
            onClick={newNote}
          >
            New note
          </Button>
          <Button
            variant="default"
            leftSection={<Headphones size={17} />}
            onClick={start}
            disabled={
              state.recording ||
              state.call_recording ||
              state.note_recording ||
              state.busy
            }
          >
            Record a conversation
          </Button>
        </Group>
      </div>
      <section
        className="thought-capture"
        aria-labelledby="thought-capture-title"
      >
        <div className="thought-capture-icon">
          <Microphone size={27} weight="duotone" />
        </div>
        <div className="thought-capture-copy">
          <h2 id="thought-capture-title">Speak your thoughts</h2>
          <p>
            Talk through an idea. Watch it become a note, with your original
            words kept alongside.
          </p>
        </div>
        <div className="thought-capture-action">
          <Button
            leftSection={<Microphone size={17} />}
            onClick={speak}
            disabled={
              state.recording ||
              state.call_recording ||
              state.note_recording ||
              state.busy ||
              state.loading
            }
          >
            Start a spoken note
          </Button>
          {state.settings.quick_note_hotkey && (
            <span>
              <Kbd>{shortcutLabel(state.settings.quick_note_hotkey)}</Kbd> from
              any app
            </span>
          )}
        </div>
      </section>
      {state.filing_status && (
        <div className="filing-state" role="status">
          <Sparkle size={17} />
          <span>{state.filing_status}</span>
        </div>
      )}
      <div className="library-tools">
        <SegmentedControl
          value={kind}
          onChange={setKind}
          data={[
            { label: "Everything", value: "all" },
            { label: "Conversations", value: "call" },
            { label: "Notes", value: "note" },
            { label: "Dictations", value: "dictation" },
          ]}
        />
        <TextInput
          aria-label="Search your notes"
          placeholder="Search your notes"
          value={query}
          onChange={(e) => setQuery(e.currentTarget.value)}
          leftSection={<MagnifyingGlass size={18} />}
          rightSection={
            query ? (
              <ActionIcon
                variant="subtle"
                aria-label="Clear search"
                onClick={() => setQuery("")}
              >
                <X size={14} />
              </ActionIcon>
            ) : null
          }
        />
      </div>
      <div className="library-layout">
        <div className="session-list">
          <div className="section-heading">
            <span>{query ? "SEARCH RESULTS" : "YOUR RECENT NOTES"}</span>
            <span>
              {filtered.length} {filtered.length === 1 ? "item" : "items"}
            </span>
          </div>
          {filtered.map((session, index) => {
            const Icon = iconFor(session.kind);
            return (
              <button
                key={session.id}
                onClick={() => void open(session.id)}
                onFocus={() => setPreview(session.id)}
                onMouseEnter={() => setPreview(session.id)}
                className={`session-card ${selected?.id === session.id ? "focused" : ""} ${index < 6 ? "arriving-card" : ""}`}
                style={{ "--arrival-delay": `${index * 24}ms` }}
              >
                <div className={`session-icon ${session.kind}`}>
                  <Icon size={23} />
                </div>
                <div className="session-card-copy">
                  <div className="session-meta">
                    {session.kind === "call"
                      ? "CONVERSATION"
                      : session.kind === "note"
                        ? session.is_collection
                          ? "TOPIC"
                          : "NOTE"
                        : "DICTATION"}
                    <span>{date(session.created_ms)}</span>
                  </div>
                  <h2>{session.title || "Untitled note"}</h2>
                  <p>{session.preview || "A fresh page for your thoughts."}</p>
                  <div className="session-card-footer">
                    {session.kind === "call" ? (
                      <>
                        <Users size={14} />
                        <span>Conversation notes</span>
                        <span className="dot-divider" />
                        <Clock size={14} />
                        <span>{duration(session.duration_ms)}</span>
                      </>
                    ) : (
                      <>
                        <TextAlignLeft size={14} />
                        <span>
                          {session.kind === "note"
                            ? session.is_collection
                              ? "Combined notes"
                              : "Personal note"
                            : "Dictated text"}
                        </span>
                      </>
                    )}
                  </div>
                </div>
                <ArrowUpRight className="session-open-icon" size={20} />
              </button>
            );
          })}
          {!filtered.length && (
            <div className="empty-state">
              <Notebook size={40} weight="duotone" />
              <h2>
                {query ? "No matching notes" : "A place for your next idea"}
              </h2>
              <p>
                {query
                  ? "Try a different word or clear your search."
                  : "Record a conversation or start a note. It will be waiting here when you need it."}
              </p>
              <Button
                variant="light"
                onClick={query ? () => setQuery("") : newNote}
              >
                {query ? "Clear search" : "Write your first note"}
              </Button>
            </div>
          )}
        </div>
        <aside className="library-preview">
          <div className="section-heading">
            <span>AT A GLANCE</span>
            <Sparkle size={16} />
          </div>
          {selected ? (
            <>
              <div className="preview-kind">
                {selected.kind === "call"
                  ? "Conversation"
                  : selected.kind === "note"
                    ? "Personal note"
                    : "Dictation"}
              </div>
              <h2>{selected.title}</h2>
              <p className="preview-text">
                {selected.preview ||
                  "Start writing to make this note your own."}
              </p>
              <Divider my="xl" />
              <div className="detail-row">
                <Clock size={17} />
                <span>{date(selected.created_ms)}</span>
              </div>
              <div className="detail-row">
                <LockSimple size={17} />
                <span>Saved on this device</span>
              </div>
              <Button
                variant="light"
                fullWidth
                rightSection={<ArrowUpRight size={16} />}
                mt="xl"
                onClick={() => void open(selected.id)}
              >
                Open {selected.kind === "call" ? "conversation" : "note"}
              </Button>
            </>
          ) : (
            <p className="muted">
              Your notes and conversations will appear here.
            </p>
          )}
          <div className="quiet-tip">
            <Notebook size={22} />
            <strong>Leave room for a thought.</strong>
            <p>
              Your own notes and the important parts of a conversation belong
              together.
            </p>
          </div>
        </aside>
      </div>
    </div>
  );
}
function Reader({
  session,
  rows = [],
  live,
  status,
  seconds,
  saveState,
  onEdit,
  locked,
  back,
  act,
  onDeleted,
  notesState,
  spokenNote,
  topics = [],
  openSession,
  moveBlocked,
  filingStatus,
  filingWorking,
  notesBusy,
}) {
  const [tab, setTab] = useState("notes"),
    [search, setSearch] = useState(""),
    [deleting, setDeleting] = useState(false),
    [renaming, setRenaming] = useState(false),
    [follow, setFollow] = useState(true),
    [moving, setMoving] = useState(null),
    [moveTarget, setMoveTarget] = useState("__separate__"),
    [movePending, setMovePending] = useState(false);
  const end = useRef(null);
  useEffect(() => {
    setTab(session?.kind === "dictation" ? "transcript" : "notes");
    setSearch("");
    setRenaming(false);
    setMoving(null);
  }, [session?.id]);
  const source = rows?.length
      ? rows
          .map((r) => `[${time(r.start_ms)}] ${r.speaker}: ${r.text}`)
          .join("\n\n")
      : session?.text || "",
    content = tab === "notes" ? session?.personal_notes || "" : source,
    speakers = session?.is_collection
      ? []
      : [...new Set((rows || []).map((r) => r.speaker))];
  const lastRow = rows?.at(-1);
  const tail = `${rows?.length || 0}:${lastRow?.start_ms}:${lastRow?.speaker}:${lastRow?.text}`;
  useEffect(() => {
    if (live && follow && tab === "transcript")
      end.current?.scrollIntoView({ block: "end", behavior: "smooth" });
  }, [tail, live, follow, tab]);
  const run = (command) => void act(command).catch(() => {});
  const moveSource = (source, topicId) => {
    setMoving({ ...source, topic_id: topicId ?? null });
    setMoveTarget(topicId || "__separate__");
  };
  return (
    <section className="reader">
      <div className="reader-breadcrumb">
        <Button
          variant="subtle"
          color="gray"
          leftSection={<ArrowLeft size={16} />}
          onClick={back}
        >
          All notes
        </Button>
        <span className="save-indicator" data-saving={saveState === "Saving…"}>
          <Check size={14} />
          {saveState}
        </span>
      </div>
      <header className="reader-header">
        <div className="reader-title-block">
          <div className="eyebrow">
            {live
              ? spokenNote
                ? "SPEAKING YOUR THOUGHTS"
                : "IN CONVERSATION"
              : session?.kind === "dictation"
                ? "DICTATION"
                : session?.kind === "note"
                  ? session.is_collection
                    ? "TOPIC NOTE"
                    : "PERSONAL NOTE"
                  : "CONVERSATION"}
          </div>
          {renaming ? (
            <TextInput
              aria-label="Note title"
              disabled={locked}
              autoFocus
              value={session?.title || ""}
              onChange={(e) => onEdit({ title: e.currentTarget.value })}
              onBlur={() => setRenaming(false)}
              onKeyDown={(e) => e.key === "Enter" && setRenaming(false)}
              size="xl"
            />
          ) : (
            <button
              className="editable-title"
              disabled={locked}
              onClick={() => setRenaming(true)}
              title="Rename"
            >
              <h1>
                {session?.title ||
                  (live
                    ? spokenNote
                      ? "Your thoughts"
                      : "Your conversation"
                    : "Opening note…")}
              </h1>
            </button>
          )}
          <div className="reader-meta">
            {live ? (
              <>
                <Badge
                  color="mint"
                  variant="light"
                  leftSection={<Record weight="fill" size={10} />}
                >
                  Recording
                </Badge>
                <span>{time(seconds * 1000)}</span>
              </>
            ) : (
              <>
                <span>{session ? date(session.created_ms) : "Loading…"}</span>
                <span className="dot-divider" />
                <span>
                  {session?.kind === "dictation"
                    ? "Dictated text"
                    : session?.kind === "note"
                      ? "Personal note"
                      : `${speakers.length || 1} ${speakers.length === 1 ? "voice" : "voices"}`}
                </span>
              </>
            )}
            <span className="dot-divider" />
            <LockSimple size={14} />
            <span>Only on your device</span>
            {session?.topic_id && (
              <Button
                variant="subtle"
                size="compact-xs"
                onClick={() => openSession(session.topic_id)}
              >
                Filed in{" "}
                {topics.find((topic) => topic.id === session.topic_id)?.title ||
                  "a topic"}
              </Button>
            )}
          </div>
        </div>
        <Group gap={8}>
          {live && (
            <Button
              leftSection={<Stop size={16} weight="fill" />}
              onClick={() =>
                run({ type: spokenNote ? "note_capture_stop" : "call_stop" })
              }
            >
              {spokenNote ? "Finish note" : "Finish recording"}
            </Button>
          )}
          <Menu withinPortal position="bottom-end">
            <Menu.Target>
              <ActionIcon
                variant="default"
                size={38}
                aria-label="More note actions"
              >
                <DotsThree size={23} />
              </ActionIcon>
            </Menu.Target>
            <Menu.Dropdown>
              <Menu.Item disabled={locked} onClick={() => setRenaming(true)}>
                Rename
              </Menu.Item>
              {!session?.is_collection &&
                ["note", "dictation"].includes(session?.kind) &&
                source.trim() &&
                topics.length > 0 && (
                  <Menu.Item
                    disabled={live || moveBlocked}
                    onClick={() => moveSource(session, session.topic_id)}
                  >
                    {session.topic_id
                      ? "Move to another topic"
                      : "File in a topic"}
                  </Menu.Item>
                )}
              {!session?.is_collection &&
                !session?.topic_id &&
                source.trim() &&
                ["note", "dictation"].includes(session?.kind) && (
                  <Menu.Item
                    disabled={live || moveBlocked}
                    onClick={() => run({ type: "source_file", id: session.id })}
                  >
                    Find a topic automatically
                  </Menu.Item>
                )}
              <Menu.Item
                leftSection={<DownloadSimple size={16} />}
                disabled={tab === "sources"}
                onClick={() =>
                  run({ type: "export", text: content, extension: "txt" })
                }
              >
                Export as text
              </Menu.Item>
              <Menu.Divider />
              <Menu.Item
                color="red"
                leftSection={<Trash size={16} />}
                disabled={live || !session}
                onClick={() => setDeleting(true)}
              >
                Delete note
              </Menu.Item>
            </Menu.Dropdown>
          </Menu>
        </Group>
      </header>
      {session?.audio_packets_lost > 0 && (
        <Alert color="yellow" variant="light" role="status" my="sm">
          Call audio may have been interrupted. Review the transcript for gaps.
        </Alert>
      )}
      {filingStatus && (
        <div className="filing-state" role="status">
          <Sparkle size={17} />
          <span>{filingStatus}</span>
          {filingWorking && (
            <Badge variant="light" size="sm">
              Organizing
            </Badge>
          )}
        </div>
      )}
      <Tabs className="reader-tabs" value={tab} onChange={setTab}>
        <div className="reader-toolbar">
          <Tabs.List>
            <Tabs.Tab value="notes" leftSection={<Notebook size={18} />}>
              Notes
            </Tabs.Tab>
            {!session?.is_collection && (
              <Tabs.Tab
                value="transcript"
                leftSection={<TextAlignLeft size={18} />}
              >
                Transcript
              </Tabs.Tab>
            )}
            {session?.is_collection && (
              <Tabs.Tab value="sources" leftSection={<Microphone size={18} />}>
                Dictations{" "}
                <Badge size="xs" variant="light" ml={7}>
                  {session.sources?.length || 0}
                </Badge>
              </Tabs.Tab>
            )}
          </Tabs.List>
          <Group gap={8}>
            {tab === "transcript" &&
              session?.kind === "dictation" &&
              !rows?.length && (
                <TranscriptEditor
                  key={session.id}
                  session={session}
                  act={act}
                  disabled={live || locked}
                />
              )}
            {tab === "notes" && (session?.kind !== "note" || source.trim()) && (
              <Button
                size="xs"
                variant="light"
                leftSection={<Sparkle size={15} />}
                onClick={() => run({ type: "notes_generate", id: session?.id })}
                disabled={!session || notesState?.working || notesBusy}
                loading={notesState?.working}
              >
                Update notes
              </Button>
            )}
            <Tooltip label={`Copy ${tab}`}>
              <ActionIcon
                size={34}
                variant="subtle"
                color="gray"
                aria-label={`Copy ${tab}`}
                disabled={!content || tab === "sources"}
                onClick={() => run({ type: "copy", text: content })}
              >
                <Copy size={18} />
              </ActionIcon>
            </Tooltip>
            <Tooltip label={`Export ${tab}`}>
              <ActionIcon
                size={34}
                variant="subtle"
                color="gray"
                aria-label={`Export ${tab}`}
                disabled={!content || tab === "sources"}
                onClick={() =>
                  run({ type: "export", text: content, extension: "txt" })
                }
              >
                <DownloadSimple size={19} />
              </ActionIcon>
            </Tooltip>
          </Group>
        </div>
        <div className="reader-body">
          <div className="document-pane">
            <Tabs.Panel value="notes" className="document-panel">
              <div className="document-caption">
                <span>
                  {live
                    ? spokenNote
                      ? "Your ideas, taking shape"
                      : "Notes grow with the conversation"
                    : "YOUR NOTES"}
                </span>
                <span>{words(session?.personal_notes || "")} words</span>
              </div>
              {notesState?.status && (
                <p className="muted">{notesState.status}</p>
              )}
              <Textarea
                aria-label="Notes document"
                classNames={{ root: "note-editor", input: "note-editor-input" }}
                autosize
                minRows={16}
                variant="unstyled"
                placeholder={
                  live
                    ? "Important details will appear here. You can add your own thoughts at any time."
                    : "A little space to think. Start writing…"
                }
                value={session?.personal_notes || ""}
                disabled={!session || locked}
                onChange={(e) =>
                  onEdit({ personal_notes: e.currentTarget.value })
                }
              />
              <p className="document-footnote">
                <LockSimple size={14} /> Changes save automatically.
              </p>
            </Tabs.Panel>
            <Tabs.Panel value="transcript" className="transcript-panel">
              <div className="transcript-tools">
                <TextInput
                  aria-label="Find in transcript"
                  placeholder="Find a word or a speaker"
                  leftSection={<MagnifyingGlass size={17} />}
                  value={search}
                  onChange={(e) => setSearch(e.currentTarget.value)}
                  size="sm"
                />
                {live && (
                  <Button
                    variant={follow ? "light" : "subtle"}
                    size="xs"
                    onClick={() => setFollow(!follow)}
                  >
                    {follow ? "Following live" : "Follow live"}
                  </Button>
                )}
              </div>
              {rows?.length ? (
                rows
                  .filter((r) =>
                    `${r.speaker} ${r.text}`
                      .toLowerCase()
                      .includes(search.toLowerCase()),
                  )
                  .map((row, index) => (
                    <article
                      className={`transcript-row ${row.provisional ? "provisional" : ""}`}
                      key={`${row.start_ms}-${row.speaker}-${index}`}
                    >
                      <Avatar
                        radius="xl"
                        size={32}
                        color={
                          ["mint", "blue", "grape"][
                            Math.max(0, speakers.indexOf(row.speaker)) % 3
                          ]
                        }
                      >
                        {row.speaker?.slice(0, 1) || "?"}
                      </Avatar>
                      <div>
                        <div className="speaker-line">
                          <strong>{row.speaker}</strong>
                          <time>{time(row.start_ms)}</time>
                          {row.provisional && <span>Refining…</span>}
                        </div>
                        <p>{row.text}</p>
                        {row.cues?.length > 0 && (
                          <Group gap="xs" mt="xs" aria-label="Sound cues">
                            {row.cues.map((cue) => (
                              <Badge
                                key={`${cue.start_ms}-${cue.label}`}
                                variant="light"
                                color="mint"
                                title={`Detected within ${time(cue.start_ms)} to ${time(cue.end_ms)}`}
                              >
                                {cue.label} · {time(cue.start_ms)}
                              </Badge>
                            ))}
                          </Group>
                        )}
                      </div>
                    </article>
                  ))
              ) : (
                <div className="plain-transcript">
                  {source ||
                    (live
                      ? "Listening for the first words…"
                      : "There is no transcript attached to this note.")}
                </div>
              )}
              <div ref={end} />
            </Tabs.Panel>
            <Tabs.Panel value="sources" className="source-panel">
              <div className="document-caption">
                <span>THE THOUGHTS BEHIND THIS NOTE</span>
                <span>
                  {session?.sources?.length || 0}{" "}
                  {session?.sources?.length === 1 ? "dictation" : "dictations"}
                </span>
              </div>
              <p className="muted">
                Each dictation keeps its original words. Move one to another
                topic whenever you need.
              </p>
              {(session?.sources || []).map((source) => (
                <article className="source-entry" key={source.id}>
                  <div className="source-entry-heading">
                    <Microphone size={19} />
                    <strong>{source.title}</strong>
                    <time>{date(source.created_ms)}</time>
                  </div>
                  <p>{source.preview}</p>
                  <Group gap={8}>
                    <Button
                      variant="subtle"
                      size="sm"
                      onClick={() => openSession(source.id)}
                    >
                      Read dictation
                    </Button>
                    <Button
                      variant="default"
                      size="sm"
                      disabled={moveBlocked}
                      onClick={() => moveSource(source, session.id)}
                    >
                      Move to topic
                    </Button>
                  </Group>
                </article>
              ))}
              {!session?.sources?.length && (
                <p className="muted">
                  Dictations filed here will appear alongside this document.
                </p>
              )}
            </Tabs.Panel>
          </div>
          <aside className="people-pane">
            <div className="section-heading">
              <span>
                {session?.kind === "note" ? "DETAILS" : "IN THIS CONVERSATION"}
              </span>
            </div>
            {speakers.map((speaker, index) => (
              <div className="person" key={speaker}>
                <Avatar
                  size={34}
                  radius="xl"
                  color={["mint", "blue", "grape"][index % 3]}
                >
                  {speaker.slice(0, 1)}
                </Avatar>
                <span>{speaker}</span>
                {speaker === "You" && (
                  <Badge size="xs" color="gray" variant="light">
                    YOU
                  </Badge>
                )}
              </div>
            ))}
            <Divider my="xl" />
            <div className="detail-row">
              <Notebook size={17} />
              <span>{words(session?.personal_notes || "")} words of notes</span>
            </div>
            <div className="detail-row">
              <LockSimple size={17} />
              <span>Private to this device</span>
            </div>
            {live && <p className="muted">{status}</p>}
          </aside>
        </div>
      </Tabs>
      <Modal
        opened={Boolean(moving)}
        onClose={() => !movePending && setMoving(null)}
        title="Move this dictation"
        centered
        size="md"
      >
        <p className="muted">
          Move “{moving?.title}” while keeping its original words. The topic
          notes will update to reflect their remaining dictations.
        </p>
        <Select
          label="Topic"
          searchable
          allowDeselect={false}
          data={[
            { value: "__separate__", label: "Keep as a separate note" },
            ...topics.map((topic) => ({ value: topic.id, label: topic.title })),
          ]}
          value={moveTarget}
          onChange={(value) => setMoveTarget(value ?? "__separate__")}
          disabled={movePending}
          nothingFoundMessage="No matching topics"
        />
        {moveBlocked && (
          <p className="muted">
            Moving is available when recording and note updates finish.
          </p>
        )}
        <Group justify="flex-end" mt="xl">
          <Button
            variant="default"
            disabled={movePending}
            onClick={() => setMoving(null)}
          >
            Cancel
          </Button>
          <Button
            loading={movePending}
            disabled={
              moveBlocked || moveTarget === (moving?.topic_id || "__separate__")
            }
            onClick={async () => {
              setMovePending(true);
              try {
                await act(
                  sourceMoveAction(
                    moving.id,
                    moveTarget === "__separate__" ? null : moveTarget,
                    moving.topic_id,
                  ),
                );
                setMoving(null);
              } catch {
              } finally {
                setMovePending(false);
              }
            }}
          >
            Move dictation
          </Button>
        </Group>
      </Modal>
      <Modal
        opened={deleting}
        onClose={() => setDeleting(false)}
        title="Delete this note?"
        size="sm"
      >
        <p>The saved transcript and notes will be removed from this device.</p>
        <Group justify="flex-end" mt="xl">
          <Button variant="default" onClick={() => setDeleting(false)}>
            Keep note
          </Button>
          <Button
            color="red"
            onClick={() =>
              void act({ type: "session_delete", id: session.id })
                .then(() => {
                  setDeleting(false);
                  onDeleted();
                })
                .catch(() => {})
            }
          >
            Delete note
          </Button>
        </Group>
      </Modal>
    </section>
  );
}
function Dictation({ state, act, open, settings, binding }) {
  const recent = state.history
      .filter((s) => s.kind === "dictation")
      .slice(0, 8),
    run = (command) => void act(command).catch(() => {});
  return (
    <div className="page-content dictation-page">
      <div className="page-header">
        <div>
          <div className="eyebrow">MAKE ROOM FOR YOUR WORDS</div>
          <h1 className="page-heading">Dictation</h1>
          <p className="page-subtitle">
            Think out loud. Keep your train of thought.
          </p>
        </div>
        <Badge variant="light" color={state.ready ? "mint" : "gray"}>
          {state.ready ? "Ready to listen" : "Setup needed"}
        </Badge>
      </div>
      <section className="dictation-stage">
        <div className={`mic-emblem ${state.recording ? "listening" : ""}`}>
          <Microphone size={35} weight="duotone" />
        </div>
        <h2>
          {state.recording
            ? "Go ahead. I’m listening."
            : state.busy
              ? "A moment for the finishing touches."
              : "Your next thought starts here."}
        </h2>
        <p>
          {state.recording
            ? time(state.seconds * 1000)
            : state.ready
              ? "Hold your shortcut in any text field, then speak naturally."
              : "Set up your speech model to start dictating on this device."}
        </p>
        {state.ready ? (
          <Group justify="center">
            <Button
              size="md"
              leftSection={
                state.recording ? <Stop size={17} /> : <Microphone size={17} />
              }
              disabled={state.busy || state.note_recording}
              onClick={() =>
                run({
                  type: state.recording ? "dictation_stop" : "dictation_start",
                })
              }
            >
              {state.recording ? "Finish dictation" : "Start dictating"}
            </Button>
            {state.busy && (
              <Button
                variant="subtle"
                color="gray"
                onClick={() => run({ type: "dictation_cancel" })}
              >
                Cancel
              </Button>
            )}
          </Group>
        ) : (
          <Button onClick={settings}>Set up dictation</Button>
        )}
        <div className="dictation-shortcut">
          <Kbd>{binding}</Kbd>
          <span>
            {state.settings.hotkey_mode === "Hold"
              ? "Hold to speak"
              : "Press to start"}
          </span>
        </div>
      </section>
      {(state.text || state.recording) && (
        <section className="dictation-result">
          <Group justify="space-between">
            <div className="section-heading">
              {state.recording ? "LIVE TRANSCRIPT" : "YOUR WORDS, READY TO USE"}
            </div>
            <Group gap="xs">
              <TranscriptEditor
                key={state.dictation_session?.id || "latest"}
                session={state.dictation_session}
                act={act}
                disabled={
                  state.recording ||
                  state.busy ||
                  state.dictation_session?.text !== state.text
                }
              />
              <Button
                variant="subtle"
                size="xs"
                leftSection={<Copy size={16} />}
                onClick={() => run({ type: "copy", text: state.text })}
              >
                Copy text
              </Button>
            </Group>
          </Group>
          <p>{state.text || "Listening…"}</p>
          <span className="muted">{state.status}</span>
        </section>
      )}
      <div className="section-heading recent-heading">
        <span>RECENT DICTATIONS</span>
        <span>{recent.length} saved</span>
      </div>
      <div className="recent-dictations">
        {recent.map((session) => (
          <button key={session.id} onClick={() => void open(session.id)}>
            <span>
              {new Intl.DateTimeFormat(undefined, {
                hour: "2-digit",
                minute: "2-digit",
              }).format(session.created_ms)}
            </span>
            <p>{session.preview}</p>
            <ArrowUpRight size={18} />
          </button>
        ))}
        {!recent.length && (
          <p className="empty-message">
            Your finished dictations will be saved here, ready to find and copy.
          </p>
        )}
      </div>
    </div>
  );
}
