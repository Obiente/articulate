// The application shell: navigation, top bar, status banners and routing to
// feature pages. Feature pages live in ./features; shared state handling in
// ./state and ./hooks.
import { useCallback, useEffect, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Avatar,
  Button,
  Kbd,
  Menu,
  Skeleton,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import {
  ArrowUpRight,
  Bell,
  BookOpen,
  ChartBar,
  Check,
  GearSix,
  Headphones,
  Lightning,
  LockSimple,
  Microphone,
  Notebook,
  Record,
} from "@phosphor-icons/react";
import * as bridge from "./bridge.js";
import { changesDocument } from "./actions.js";
import { NavHighlight } from "./motion.jsx";
import { createDesktopStore, useDesktopStore } from "./state/desktop-store.js";
import { useDocumentEdits } from "./hooks/useDocumentEdits.js";
import { useCloseHandshake } from "./hooks/useCloseHandshake.jsx";
import { dictationShortcut } from "./lib/format.js";
import { useCaptureNotices } from "./features/capture/useCaptureNotices.jsx";
import { CallSetupDialog } from "./features/calls/CallSetupDialog.jsx";
import {
  DropOverlay,
  ImportAudioDialog,
} from "./features/calls/ImportAudio.jsx";
import { DictationPage } from "./features/dictation/DictationPage.jsx";
import { InsightsPage } from "./features/insights/InsightsPage.jsx";
import { LibraryPage } from "./features/library/LibraryPage.jsx";
import { useLibraryNotices } from "./features/library/useLibraryNotices.jsx";
import { Reader } from "./features/notes/Reader.jsx";
import { SettingsPage } from "./features/settings/SettingsPage.jsx";
import {
  MeetingIndicator,
  useMeetingNotices,
} from "./features/meetings/useMeetingNotices.jsx";
import { useUpdateNotices } from "./features/updates/useUpdateNotices.jsx";
import { SnippetsPage } from "./features/vocabulary/SnippetsPage.jsx";
import { VocabularyPage } from "./features/vocabulary/VocabularyPage.jsx";

const navigation = [
  ["dictation", "Dictation", Microphone],
  ["notetaker", "Notetaker", Notebook],
  ["conversations", "Conversations", Headphones],
  ["insights", "Insights", ChartBar],
  ["vocabulary", "Vocabulary", BookOpen],
  ["snippets", "Snippets", Lightning],
];
const readerPages = ["notetaker", "conversations", "dictation"];

const desktop = createDesktopStore(bridge);
const currentState = () => desktop.getSnapshot().state;

export function App() {
  const { state, error } = useDesktopStore(desktop);
  const [page, setPage] = useState("notetaker"),
    [workspace, setWorkspace] = useState("library"),
    [callSetup, setCallSetup] = useState(false),
    [settingsTarget, setSettingsTarget] = useState(null);
  const openedCapture = useRef(null),
    contentScroll = useRef(null);
  useEffect(() => {
    contentScroll.current?.scrollTo({ top: 0 });
  }, [page, workspace, state?.selected?.id]);
  const report = useCallback(
    (e) =>
      notifications.show({
        title: "Could not complete that",
        message: String(e),
        color: "red",
      }),
    [],
  );
  const documents = useDocumentEdits({
    state,
    getState: currentState,
    sync: desktop.sync,
    report,
  });
  const { flush, beginTransition, endTransition, inTransition } = documents;
  const act = useCallback(
    async (command) => {
      const changes = changesDocument(command.type);
      if (changes && inTransition())
        throw new Error(
          "Please wait for the current document action to finish.",
        );
      if (changes) beginTransition();
      try {
        if (changes) await flush();
        await bridge.action(command);
        await desktop.sync();
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
        if (changes) endTransition();
      }
    },
    [flush, beginTransition, endTransition, inTransition, report],
  );
  const close = useCloseHandshake({ documents, report });
  const navigate = useCallback(
    async (next) => {
      try {
        await flush();
        setPage(next);
        setWorkspace("library");
      } catch (e) {
        report(e);
      }
    },
    [flush, report],
  );
  const showSettingsSection = useCallback(
    (section) => {
      setSettingsTarget(section);
      void navigate("settings");
    },
    [navigate],
  );
  useEffect(() => {
    if (page !== "settings" || !settingsTarget) return;
    const frame = requestAnimationFrame(() => {
      document
        .getElementById(`settings-${settingsTarget}`)
        ?.scrollIntoView({ block: "start", behavior: "smooth" });
      setSettingsTarget(null);
    });
    return () => cancelAnimationFrame(frame);
  }, [page, settingsTarget]);
  const notices = useUpdateNotices({ state, act, showSettingsSection });
  const meeting = useMeetingNotices({ state, act });
  // Open a saved document in the page and workspace for its kind.
  const open = useCallback(
    async (id) => {
      try {
        await act({ type: "history_open", id });
        const current = currentState();
        const kind =
          current?.history.find((item) => item.id === id)?.kind ||
          (current?.call_session?.id === id
            ? "call"
            : current?.dictation_session?.id === id
              ? "dictation"
              : "note");
        setWorkspace(
          current?.note_recording && current.note_session?.id === id
            ? "live-note"
            : current?.call_recording && current.call_session?.id === id
              ? "live"
              : "saved",
        );
        setPage(
          kind === "call"
            ? "conversations"
            : kind === "dictation"
              ? "dictation"
              : "notetaker",
        );
      } catch {}
    },
    [act],
  );
  const newNote = useCallback(async () => {
    try {
      await act({ type: "note_new" });
      setWorkspace("saved");
      setPage("notetaker");
    } catch {}
  }, [act]);
  useCaptureNotices({
    feedback: state?.capture_feedback,
    openRecording: open,
  });
  useLibraryNotices(state?.library_notice);
  // A spoken note started from the shortcut opens in Notetaker.
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
    // Not named `document`: that would shadow the DOM global used above.
    openDocument = selected
      ? { ...selected, ...documents.edits[selected.id] }
      : null;
  const saveState = error
    ? "Connection unavailable"
    : state?.history_error
      ? "Save failed"
      : documents.savingNow ||
          state?.saving ||
          Object.keys(documents.edits).length
        ? "Saving…"
        : "Saved on this device";
  const binding = dictationShortcut(state?.settings?.hotkey);
  const captureBlocked = Boolean(
    state?.recording ||
      state?.call_recording ||
      state?.note_recording ||
      state?.busy ||
      state?.loading,
  );
  const controller = state?.controller;
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
            {readerPages.includes(page) && workspace !== "library"
              ? workspace === "live-note"
                ? "Notetaker / Spoken note"
                : page === "conversations"
                  ? `Conversations / ${workspace === "live" ? "Recording" : "Saved"}`
                  : page === "dictation"
                    ? "Dictation / Saved"
                    : "Notetaker / Your notes"
              : navigation.find((n) => n[0] === page)?.[1] || "Settings"}
          </span>
          <div className="topbar-right">
            <Menu withinPortal position="bottom-end">
              <Menu.Target>
                <ActionIcon
                  className="update-notification-button"
                  variant="subtle"
                  color={notices.length ? "mint" : "gray"}
                  size={34}
                  aria-label={`${notices.length} update ${notices.length === 1 ? "notification" : "notifications"}`}
                >
                  <Bell size={19} />
                  {notices.length > 0 && (
                    <span className="update-notification-count">
                      {notices.length}
                    </span>
                  )}
                </ActionIcon>
              </Menu.Target>
              <Menu.Dropdown>
                <Menu.Label>Updates</Menu.Label>
                {notices.length ? (
                  notices.map((notice) => (
                    <Menu.Item
                      key={notice.id}
                      onClick={() => showSettingsSection(notice.section)}
                    >
                      <strong>{notice.title}</strong>
                      <span className="update-notification-detail">
                        {notice.message}
                      </span>
                    </Menu.Item>
                  ))
                ) : (
                  <Menu.Item disabled>No update alerts</Menu.Item>
                )}
              </Menu.Dropdown>
            </Menu>
            <LockSimple size={14} />
            <span>Your words stay yours</span>
            <Avatar size={29} radius="xl" color="mint">
              Y
            </Avatar>
          </div>
        </header>
        {state?.call_recording &&
          !(page === "conversations" && workspace === "live") && (
            <button
              className="recording-banner"
              onClick={() => {
                setPage("conversations");
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
          className={`main-content ${readerPages.includes(page) && workspace !== "library" ? "reader-mode" : ""}`}
        >
          <MeetingIndicator indicator={meeting} />
          {controller?.state === "failed" && (
            <Alert color="red" title="Articulate stopped working" mb="md">
              {controller.message} Your latest changes could not be verified as
              saved. Close Articulate and open it again.
              <Button
                variant="subtle"
                size="xs"
                onClick={() => void close.closeApp()}
              >
                Close Articulate
              </Button>
            </Alert>
          )}
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
          ) : page === "notetaker" ||
            page === "conversations" ||
            (page === "dictation" && workspace !== "library") ? (
            workspace === "library" ? (
              <LibraryPage
                mode={page === "conversations" ? "call" : "note"}
                history={state.history}
                search={state.search}
                recovery={state.recovery}
                act={act}
                captureBlocked={captureBlocked}
                filingStatus={state.filing_status}
                quickNoteHotkey={state.settings.quick_note_hotkey}
                open={open}
                newNote={newNote}
                speak={() =>
                  void act({ type: "note_capture_start" })
                    .then(() => setWorkspace("live-note"))
                    .catch(() => {})
                }
                start={() => setCallSetup(true)}
                act={act}
              />
            ) : (
              <Reader
                session={openDocument}
                rows={
                  workspace === "live-note"
                    ? state.note_rows || openDocument?.rows
                    : workspace === "live"
                      ? state.call_rows
                      : openDocument?.rows
                }
                live={
                  workspace === "live-note"
                    ? state.note_recording
                    : workspace === "live" && state.call_recording
                }
                spokenNote={
                  workspace === "live-note" ||
                  (openDocument?.kind === "note" && Boolean(openDocument?.text))
                }
                status={
                  workspace === "live-note"
                    ? state.note_status
                    : state.call_status
                }
                cueEnabled={workspace === "live" && state.call_cues_enabled}
                cueStatus={workspace === "live" ? state.call_cues_status : ""}
                seconds={
                  workspace === "live-note"
                    ? (state.note_seconds ?? state.seconds)
                    : state.seconds
                }
                saveState={saveState}
                onEdit={(patch) =>
                  openDocument && documents.edit(openDocument.id, patch)
                }
                locked={documents.documentLocked || state.source_move_working}
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
                people={state.people}
                playback={state.playback}
                importState={state.audio_import}
              />
            )
          ) : page === "dictation" ? (
            <DictationPage
              state={state}
              act={act}
              open={open}
              settings={() => void navigate("settings")}
              binding={binding}
            />
          ) : page === "vocabulary" ? (
            <VocabularyPage
              dictionary={state.dictionary}
              review={state.assort}
              act={act}
            />
          ) : page === "snippets" ? (
            <SnippetsPage macros={state.macros} act={act} />
          ) : page === "insights" ? (
            <InsightsPage
              insights={state.insights}
              historyLoading={state.history_loading}
              act={act}
            />
          ) : (
            <SettingsPage state={state} act={act} />
          )}
        </div>
      </main>
      <CallSetupDialog
        opened={callSetup}
        state={state}
        act={act}
        onClose={() => setCallSetup(false)}
        onStarted={() => {
          setCallSetup(false);
          setPage("conversations");
          setWorkspace("live");
        }}
      />
      <ImportAudioDialog state={state} act={act} openSession={open} />
      <DropOverlay dragging={state?.audio_import?.dragging} />
      {close.dialog}
    </div>
  );
}
