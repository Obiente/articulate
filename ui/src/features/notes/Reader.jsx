import { useEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Avatar,
  Badge,
  Button,
  Divider,
  Group,
  Menu,
  Tabs,
  Textarea,
  TextInput,
  Tooltip,
} from "@mantine/core";
import {
  ArrowLeft,
  Check,
  Copy,
  DotsThree,
  LockSimple,
  MagnifyingGlass,
  Microphone,
  Notebook,
  PushPin,
  Record,
  Sparkle,
  Stop,
  TextAlignLeft,
  Trash,
} from "@phosphor-icons/react";
import { countWords, formatDate, formatTime } from "../../lib/format.js";
import { TranscriptEditor } from "../dictation/TranscriptEditor.jsx";
import { SpeakerTimeline } from "../calls/SpeakerTimeline.jsx";
import { TranscriptRows } from "../calls/TranscriptRows.jsx";
import {
  speakerColors,
  speakersOf,
  speakerToneFor,
  transcriptText,
} from "../calls/speakers.js";
import { DeleteDocumentDialog, MoveSourceDialog } from "./DocumentDialogs.jsx";
import { ExportMenu } from "./ExportMenu.jsx";

const NO_ROWS = [];

// An open note, conversation or dictation: notes, transcript and sources.
export function Reader({
  session,
  rows = NO_ROWS,
  live,
  status,
  cueEnabled,
  cueStatus,
  seconds,
  saveState,
  onEdit,
  locked,
  back,
  act,
  onDeleted,
  notesState,
  spokenNote,
  topics = NO_ROWS,
  openSession,
  moveBlocked,
  filingStatus,
  filingWorking,
  notesBusy,
}) {
  rows ??= NO_ROWS;
  const [tab, setTab] = useState("notes"),
    [search, setSearch] = useState(""),
    [speakerFilter, setSpeakerFilter] = useState(null),
    [deleting, setDeleting] = useState(false),
    [renaming, setRenaming] = useState(false),
    [follow, setFollow] = useState(true),
    [moving, setMoving] = useState(null);
  const documentNoun =
    session?.kind === "call"
      ? "conversation"
      : session?.kind === "dictation"
        ? "dictation"
        : "note";
  const end = useRef(null);
  useEffect(() => {
    setTab(session?.kind === "dictation" ? "transcript" : "notes");
    setSearch("");
    setSpeakerFilter(null);
    setRenaming(false);
    setMoving(null);
  }, [session?.id]);
  const speakers = useMemo(
    () => (session?.is_collection ? NO_ROWS : speakersOf(rows)),
    [rows, session?.is_collection],
  );
  const speakerTone = useMemo(() => speakerToneFor(speakers), [speakers]);
  // The joined transcript is built only when copying or exporting.
  const hasTranscript = rows.length > 0 || Boolean(session?.text);
  const transcriptVisible = rows.length > 0 || Boolean(session?.text?.trim());
  const notes = session?.personal_notes || "";
  const hasContent = tab === "notes" ? Boolean(notes) : hasTranscript;
  const content = () =>
    tab === "notes" ? notes : transcriptText(rows, session?.text || "");
  const lastRow = rows.at(-1);
  const tail = `${rows.length}:${lastRow?.start_ms}:${lastRow?.speaker}:${lastRow?.text}`;
  useEffect(() => {
    if (live && follow && tab === "transcript")
      end.current?.scrollIntoView({ block: "end", behavior: "smooth" });
  }, [tail, live, follow, tab]);
  const run = (command) => void act(command).catch(() => {});
  const accepted = (command) =>
    act(command).then(
      () => true,
      () => false,
    );
  const moveSource = (source, topicId) =>
    setMoving({ ...source, topic_id: topicId ?? null });
  const noteWords = countWords(notes);
  return (
    <section className="reader">
      <div className="reader-breadcrumb">
        <Button
          variant="subtle"
          color="gray"
          leftSection={<ArrowLeft size={16} />}
          onClick={back}
        >
          {session?.kind === "call"
            ? "All conversations"
            : session?.kind === "dictation"
              ? "All dictations"
              : "All notes"}
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
                <span>{formatTime(seconds * 1000)}</span>
              </>
            ) : (
              <>
                <span>
                  {session ? formatDate(session.created_ms) : "Loading…"}
                </span>
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
          {!live && session && (
            <Tooltip label={session.pinned ? "Unpin" : "Pin"}>
              <ActionIcon
                variant={session.pinned ? "light" : "default"}
                size={38}
                aria-label={session.pinned ? "Unpin" : "Pin"}
                aria-pressed={Boolean(session.pinned)}
                onClick={() =>
                  run({
                    type: "session_pin",
                    id: session.id,
                    pinned: !session.pinned,
                  })
                }
              >
                <PushPin
                  size={19}
                  weight={session.pinned ? "fill" : "regular"}
                />
              </ActionIcon>
            </Tooltip>
          )}
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
                aria-label="More document actions"
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
                transcriptVisible &&
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
                transcriptVisible &&
                ["note", "dictation"].includes(session?.kind) && (
                  <Menu.Item
                    disabled={live || moveBlocked}
                    onClick={() => run({ type: "source_file", id: session.id })}
                  >
                    Find a topic automatically
                  </Menu.Item>
                )}
              <Menu.Divider />
              <Menu.Item
                color="red"
                leftSection={<Trash size={16} />}
                disabled={live || !session}
                onClick={() => setDeleting(true)}
              >
                Delete {documentNoun}
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
              !rows.length && (
                <TranscriptEditor
                  key={session.id}
                  session={session}
                  act={act}
                  disabled={live || locked}
                />
              )}
            {tab === "notes" &&
              (session?.kind !== "note" || transcriptVisible) && (
                <Button
                  size="xs"
                  variant="light"
                  leftSection={<Sparkle size={15} />}
                  onClick={() =>
                    run({ type: "notes_generate", id: session?.id })
                  }
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
                disabled={!hasContent || tab === "sources"}
                onClick={() => run({ type: "copy", text: content() })}
              >
                <Copy size={18} />
              </ActionIcon>
            </Tooltip>
            <ExportMenu session={session} run={accepted} />
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
                <span>{noteWords} words</span>
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
                value={notes}
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
              {speakers.length > 1 && (
                <SpeakerTimeline
                  rows={rows}
                  speakers={speakers}
                  speakerTone={speakerTone}
                  live={live}
                  seconds={seconds}
                  speakerFilter={speakerFilter}
                  onFilter={setSpeakerFilter}
                />
              )}
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
              {cueEnabled && (
                <p className="muted" role="status">
                  {cueStatus ||
                    (live
                      ? "Sound and tone cues are on. Labels appear when a distinct cue is detected."
                      : rows.some((row) => row.cues?.length)
                        ? "Sound and tone cues were on for this call."
                        : "No distinct sound or non-neutral tone was reported for this call.")}
                </p>
              )}
              {rows.length ? (
                <TranscriptRows
                  rows={rows}
                  speakerFilter={speakerFilter}
                  search={search}
                  speakerTone={speakerTone}
                />
              ) : (
                <div className="plain-transcript">
                  {session?.text ||
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
                    <time>{formatDate(source.created_ms)}</time>
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
                {session?.kind === "call" ? "IN THIS CONVERSATION" : "DETAILS"}
              </span>
            </div>
            {speakers.map((speaker, index) => (
              <div className="person" key={speaker}>
                <Avatar
                  size={34}
                  radius="xl"
                  color={speakerColors[index % speakerColors.length]}
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
              <span>{noteWords} words of notes</span>
            </div>
            <div className="detail-row">
              <LockSimple size={17} />
              <span>Private to this device</span>
            </div>
            {live && <p className="muted">{status}</p>}
          </aside>
        </div>
      </Tabs>
      <MoveSourceDialog
        source={moving}
        topics={topics}
        blocked={moveBlocked}
        act={act}
        onClose={() => setMoving(null)}
      />
      <DeleteDocumentDialog
        opened={deleting}
        noun={documentNoun}
        onClose={() => setDeleting(false)}
        onConfirm={() =>
          void act({ type: "session_delete", id: session.id })
            .then(() => {
              setDeleting(false);
              onDeleted();
            })
            .catch(() => {})
        }
      />
    </section>
  );
}
