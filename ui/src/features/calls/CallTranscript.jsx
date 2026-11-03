import { useEffect, useMemo, useState } from "react";
import { ActionIcon, Autocomplete, Button, Menu } from "@mantine/core";
import { Stop, UserPlus, X } from "@phosphor-icons/react";
import { formatTime } from "../../lib/format.js";
import { TranscriptRows } from "./TranscriptRows.jsx";
import { nameSuggestions, playingRowIndex } from "./speakers.js";

// Select turns, then assign them to a speaker or a new person.
function ReassignBar({ count, roster, people, onAssign, onClear, busy }) {
  const [naming, setNaming] = useState(false),
    [name, setName] = useState("");
  useEffect(() => {
    if (!count) setNaming(false);
  }, [count]);
  const submit = (value) => {
    const clean = value.trim();
    if (clean) void onAssign({ name: clean }).then(() => setName(""));
  };
  return (
    <div className="reassign-bar" role="toolbar" aria-label="Selected turns">
      <span>
        {count} {count === 1 ? "turn" : "turns"} selected
      </span>
      {naming ? (
        <Autocomplete
          aria-label="Name of the person who spoke"
          placeholder="Who spoke?"
          size="xs"
          autoFocus
          data={nameSuggestions(people, [])}
          value={name}
          onChange={setName}
          onOptionSubmit={submit}
          onKeyDown={(event) => {
            if (event.key === "Enter") submit(name);
            if (event.key === "Escape") setNaming(false);
          }}
          disabled={busy}
          comboboxProps={{ withinPortal: true }}
        />
      ) : (
        <Menu withinPortal position="top">
          <Menu.Target>
            <Button size="xs" loading={busy}>
              Assign to…
            </Button>
          </Menu.Target>
          <Menu.Dropdown>
            {roster.map((speaker) => (
              <Menu.Item
                key={speaker.key}
                onClick={() => void onAssign({ speaker: speaker.key })}
              >
                {speaker.label}
              </Menu.Item>
            ))}
            <Menu.Divider />
            <Menu.Item
              leftSection={<UserPlus size={15} />}
              onClick={() => setNaming(true)}
            >
              Someone else…
            </Menu.Item>
          </Menu.Dropdown>
        </Menu>
      )}
      <ActionIcon
        variant="subtle"
        color="gray"
        size={28}
        aria-label="Clear selection"
        onClick={onClear}
      >
        <X size={15} />
      </ActionIcon>
    </div>
  );
}

// A conversation transcript with speaker correction and playback of kept
// audio. Saved and live conversations share it; live turns that are still
// being refined cannot be selected.
export function CallTranscript({
  session,
  rows,
  live,
  speakerFilter,
  search,
  speakerTone,
  roster,
  people,
  playback,
  act,
  locked,
}) {
  const [selected, setSelected] = useState(() => new Map()),
    [busy, setBusy] = useState(false);
  // A different document or a changed transcript clears the selection.
  useEffect(() => setSelected(new Map()), [session?.id]);
  useEffect(() => {
    setSelected((current) => {
      let changed = false;
      const next = new Map();
      for (const [index, start] of current) {
        if (rows[index]?.start_ms === start && !rows[index].provisional)
          next.set(index, start);
        else changed = true;
      }
      return changed ? next : current;
    });
  }, [rows]);
  const editable = session?.kind === "call" && !locked;
  const selectedIndexes = useMemo(() => new Set(selected.keys()), [selected]);
  const playing = playback?.session_id === session?.id && playback?.playing;
  const canPlay = Boolean(session?.audio) && !live;
  const playingIndex = playing
    ? playingRowIndex(rows, playback.position_ms)
    : -1;
  const toggle = (index, row) =>
    setSelected((current) => {
      const next = new Map(current);
      if (next.has(index)) next.delete(index);
      else next.set(index, row.start_ms);
      return next;
    });
  const assign = async (target) => {
    setBusy(true);
    try {
      await act({
        type: "speaker_reassign",
        id: session.id,
        rows: [...selected].map(([index, start_ms]) => ({ index, start_ms })),
        ...target,
      });
      setSelected(new Map());
    } catch {
      // The shell shows the error; keep the selection to retry.
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      {playing && (
        <div className="playback-bar" role="status">
          <span className="playback-dot" />
          <span>Playing from the kept recording</span>
          <time>{formatTime(playback.position_ms)}</time>
          <Button
            size="compact-xs"
            variant="subtle"
            leftSection={<Stop size={12} weight="fill" />}
            onClick={() => void act({ type: "audio_stop" }).catch(() => {})}
          >
            Stop
          </Button>
        </div>
      )}
      {playback?.error && playback.session_id !== session?.id && canPlay && (
        <p className="muted" role="status">
          {playback.error}
        </p>
      )}
      <TranscriptRows
        rows={rows}
        speakerFilter={speakerFilter}
        search={search}
        speakerTone={speakerTone}
        interruptions={session?.interruptions}
        selectable={editable}
        selected={selectedIndexes}
        onToggle={toggle}
        onPlay={
          canPlay
            ? (row) =>
                void act({
                  type: "audio_play",
                  id: session.id,
                  from_ms: row.start_ms,
                }).catch(() => {})
            : undefined
        }
        playingIndex={playingIndex}
      />
      {selected.size > 0 && (
        <ReassignBar
          count={selected.size}
          roster={roster}
          people={people}
          busy={busy}
          onAssign={assign}
          onClear={() => setSelected(new Map())}
        />
      )}
    </>
  );
}
