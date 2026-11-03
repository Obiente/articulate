import { memo, useMemo, useState } from "react";
import {
  ActionIcon,
  Button,
  Divider,
  Group,
  Kbd,
  TextInput,
} from "@mantine/core";
import {
  ArrowUpRight,
  Clock,
  Headphones,
  LockSimple,
  MagnifyingGlass,
  Microphone,
  Notebook,
  Plus,
  PushPin,
  Record,
  Sparkle,
  TextAlignLeft,
  Users,
  X,
} from "@phosphor-icons/react";
import { formatDate, formatDuration, shortcutLabel } from "../../lib/format.js";
import { pinnedFirst } from "./pins.js";
import "./library.css";
import { RecoveryBanner, RecoveryDialog } from "./Recovery.jsx";
import {
  SearchFilters,
  SearchResults,
  useLibrarySearch,
} from "./SearchResults.jsx";
import { ImportAudioButton } from "../calls/ImportAudio.jsx";

const iconFor = (kind) =>
  kind === "call" ? Headphones : kind === "note" ? Notebook : Microphone;

const SessionCard = memo(function SessionCard({
  session,
  index,
  focused,
  open,
  setPreview,
}) {
  const Icon = iconFor(session.kind);
  return (
    <button
      onClick={() => void open(session.id)}
      onFocus={() => setPreview(session.id)}
      onMouseEnter={() => setPreview(session.id)}
      className={`session-card ${focused ? "focused" : ""} ${index < 6 ? "arriving-card" : ""}`}
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
          <span>{formatDate(session.created_ms)}</span>
          {session.pinned && (
            <PushPin
              className="pinned-mark"
              size={13}
              weight="fill"
              aria-label="Pinned"
            />
          )}
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
              <span>{formatDuration(session.duration_ms)}</span>
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
});

// Notetaker (notes) or Conversations (calls): capture and saved documents.
export function LibraryPage({
  mode,
  history,
  search: searchState,
  recovery,
  act,
  captureBlocked,
  filingStatus,
  quickNoteHotkey,
  open,
  newNote,
  start,
  speak,
  act,
}) {
  const [preview, setPreview] = useState(null),
    [reviewing, setReviewing] = useState(false);
  const search = useLibrarySearch({
    act,
    search: searchState,
    defaultKind: mode,
  });
  const { query, setQuery } = search;
  const searching = search.view.active;
  const filtered = useMemo(
    () => pinnedFirst(history.filter((s) => s.kind === mode && !s.topic_id)),
    [history, mode],
  );
  const selected =
    (preview && history.find((s) => s.id === preview)) ||
    (searching ? null : filtered[0]);
  const run = (command) =>
    act(command).then(
      () => true,
      () => false,
    );
  return (
    <div className="page-content library-page">
      <div className="page-header">
        <div>
          <div className="eyebrow">
            {mode === "call" ? "EVERY VOICE IN CONTEXT" : "SPACE TO THINK"}
          </div>
          <h1 className="page-heading">
            {mode === "call" ? "Conversations" : "Notetaker"}
          </h1>
          <p className="page-subtitle">
            {mode === "call"
              ? "Meetings and calls, wherever they happen."
              : "A home for your notes and thoughts."}
          </p>
        </div>
        <Group gap={10}>
          {mode === "call" && act && (
            <ImportAudioButton act={act} disabled={captureBlocked} />
          )}
          {mode === "note" && (
            <Button
              variant="default"
              leftSection={<Plus size={17} />}
              onClick={newNote}
            >
              New note
            </Button>
          )}
        </Group>
      </div>
      {mode === "note" && (
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
              disabled={captureBlocked}
            >
              Start a spoken note
            </Button>
            {quickNoteHotkey && (
              <span>
                <Kbd>{shortcutLabel(quickNoteHotkey)}</Kbd> from any app
              </span>
            )}
          </div>
        </section>
      )}
      {mode === "call" && (
        <section className="thought-capture conversation-capture">
          <div className="thought-capture-icon">
            <Headphones size={27} weight="duotone" />
          </div>
          <div className="thought-capture-copy">
            <h2>Capture a conversation</h2>
            <p>
              Record your microphone and meeting audio from any app. Choose
              separate Discord tracks when connected.
            </p>
          </div>
          <div className="thought-capture-action">
            <Button
              leftSection={<Record size={17} />}
              onClick={start}
              disabled={captureBlocked}
            >
              Record a conversation
            </Button>
          </div>
        </section>
      )}
      {mode === "note" && filingStatus && (
        <div className="filing-state" role="status">
          <Sparkle size={17} />
          <span>{filingStatus}</span>
        </div>
      )}
      <RecoveryBanner recovery={recovery} onReview={() => setReviewing(true)} />
      <div className="library-tools">
        <span className="section-heading">
          {mode === "call" ? "SAVED CONVERSATIONS" : "SAVED NOTES"}
        </span>
        <Group gap={8} wrap="nowrap" className="library-search">
          <TextInput
            aria-label={
              mode === "call" ? "Search conversations" : "Search your notes"
            }
            placeholder={
              mode === "call"
                ? "Search words, speakers and notes"
                : "Search your notes"
            }
            value={query}
            onChange={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={(e) => e.key === "Escape" && setQuery("")}
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
          <SearchFilters
            filters={search.filters}
            setFilters={search.setFilters}
            count={search.filterCount}
            problem={search.problem}
            defaultKind={mode}
          />
        </Group>
      </div>
      <div className="library-layout">
        {searching ? (
          <div className="session-list">
            <SearchResults
              view={search.view}
              history={history}
              open={open}
              setPreview={setPreview}
              clear={search.clear}
              onRecovery={() => setReviewing(true)}
            />
          </div>
        ) : (
          <div className="session-list">
            <div className="section-heading">
              <span>
                {mode === "call" ? "RECENT CONVERSATIONS" : "YOUR RECENT NOTES"}
              </span>
              <span>
                {filtered.length} {filtered.length === 1 ? "item" : "items"}
              </span>
            </div>
            {filtered.map((session, index) => (
              <SessionCard
                key={session.id}
                session={session}
                index={index}
                focused={selected?.id === session.id}
                open={open}
                setPreview={setPreview}
              />
            ))}
            {!filtered.length && (
              <div className="empty-state">
                <Notebook size={40} weight="duotone" />
                <h2>
                  {mode === "call"
                    ? "Your conversations start here"
                    : "A place for your next idea"}
                </h2>
                <p>
                  {mode === "call"
                    ? "Record a meeting or call in any app. Its transcript will appear here."
                    : "Write or speak a note. It will be waiting here when you need it."}
                </p>
                <Button
                  variant="light"
                  onClick={mode === "call" ? start : newNote}
                >
                  {mode === "call"
                    ? "Record a conversation"
                    : "Write your first note"}
                </Button>
              </div>
            )}
          </div>
        )}
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
                <span>{formatDate(selected.created_ms)}</span>
              </div>
              <div className="detail-row">
                <LockSimple size={17} />
                <span>Saved on this device</span>
              </div>
              {selected.pinned && (
                <div className="detail-row">
                  <PushPin size={17} weight="fill" />
                  <span>Pinned, so retention rules keep it</span>
                </div>
              )}
              <Button
                variant="light"
                fullWidth
                rightSection={<ArrowUpRight size={16} />}
                mt="xl"
                onClick={() => void open(selected.id)}
              >
                Open {selected.kind === "call" ? "conversation" : "note"}
              </Button>
              <Button
                variant="subtle"
                color="gray"
                fullWidth
                mt="xs"
                leftSection={
                  <PushPin
                    size={16}
                    weight={selected.pinned ? "fill" : "regular"}
                  />
                }
                aria-pressed={Boolean(selected.pinned)}
                onClick={() =>
                  void run({
                    type: "session_pin",
                    id: selected.id,
                    pinned: !selected.pinned,
                  })
                }
              >
                {selected.pinned ? "Unpin" : "Pin to the top"}
              </Button>
            </>
          ) : (
            <p className="muted">
              {searching
                ? "Point to a result to see it here."
                : mode === "call"
                  ? "Your conversations will appear here."
                  : "Your notes will appear here."}
            </p>
          )}
          <div className="quiet-tip">
            {mode === "call" ? (
              <Headphones size={22} />
            ) : (
              <Notebook size={22} />
            )}
            <strong>
              {mode === "call"
                ? "Keep every voice in view."
                : "Leave room for a thought."}
            </strong>
            <p>
              {mode === "call"
                ? "Return to a conversation, its speakers, and the moments that mattered."
                : "Your ideas and spoken notes stay together here."}
            </p>
          </div>
        </aside>
      </div>
      <RecoveryDialog
        opened={reviewing}
        onClose={() => setReviewing(false)}
        recovery={recovery}
        run={run}
      />
    </div>
  );
}
