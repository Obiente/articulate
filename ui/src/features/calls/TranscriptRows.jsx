import { Fragment, memo, useMemo } from "react";
import { Avatar, Badge, Group, Tooltip } from "@mantine/core";
import { Check, Play, WarningCircle } from "@phosphor-icons/react";
import { formatTime } from "../../lib/format.js";
import { useVirtualList } from "../../hooks/useVirtualList.js";
import { rowSpeakers, withInterruptions } from "./speakers.js";
import "./calls.css";

const TranscriptRow = memo(function TranscriptRow({
  row,
  index,
  tone,
  selectable,
  selected,
  onToggle,
  onPlay,
  playing,
}) {
  const committed = !row.provisional;
  return (
    <article
      className={`transcript-row ${row.provisional ? "provisional" : ""} ${selected ? "selected" : ""} ${playing ? "playing" : ""}`}
      aria-current={playing ? "true" : undefined}
    >
      {selectable && committed ? (
        <Tooltip
          label={selected ? "Deselect this turn" : "Select to reassign"}
          withinPortal
          openDelay={400}
        >
          <button
            type="button"
            className="transcript-row-select"
            aria-pressed={selected}
            aria-label={`${selected ? "Deselect" : "Select"} the turn at ${formatTime(row.start_ms)} by ${row.speaker}`}
            onClick={() => onToggle(index, row)}
          >
            <Avatar radius="xl" size={32} color={tone}>
              {selected ? (
                <Check size={16} weight="bold" />
              ) : (
                row.speaker?.slice(0, 1) || "?"
              )}
            </Avatar>
          </button>
        </Tooltip>
      ) : (
        <Avatar radius="xl" size={32} color={tone}>
          {row.speaker?.slice(0, 1) || "?"}
        </Avatar>
      )}
      <div>
        <div className="speaker-line">
          <strong>{row.speaker}</strong>
          {onPlay ? (
            <button
              type="button"
              className="transcript-time-play"
              aria-label={`Play from ${formatTime(row.start_ms)}`}
              onClick={() => onPlay(row)}
            >
              <Play size={11} weight="fill" />
              <time>{formatTime(row.start_ms)}</time>
            </button>
          ) : (
            <time>{formatTime(row.start_ms)}</time>
          )}
          {row.provisional && <span>Refining…</span>}
        </div>
        <p>{row.text}</p>
        {row.cues?.length > 0 && (
          <Group gap="xs" mt="xs" aria-label="Sound and tone cues">
            {row.cues.map((cue) => (
              <Badge
                key={`${cue.start_ms}-${cue.label}`}
                variant="light"
                color={cue.label.endsWith(" tone") ? "yellow" : "mint"}
                title={`${cue.label.endsWith(" tone") ? "Possible vocal tone" : "Sound cue"} within ${formatTime(cue.start_ms)} to ${formatTime(cue.end_ms)}`}
              >
                {cue.label} · {formatTime(cue.start_ms)}
              </Badge>
            ))}
          </Group>
        )}
      </div>
    </article>
  );
});

// A subtle inline note where audio could not be recorded.
const InterruptionMarker = memo(function InterruptionMarker({ gap }) {
  return (
    <div
      className={`transcript-gap ${gap.ongoing ? "ongoing" : ""}`}
      role="note"
    >
      <WarningCircle size={14} />
      <span>{gap.message}</span>
    </div>
  );
});

// Transcript rows, filtered by speaker and search text, with interruption
// markers between turns. Long transcripts render only the rows near the
// visible area.
export function TranscriptRows({
  rows,
  speakerFilter,
  search,
  speakerTone,
  interruptions,
  selectable = false,
  selected,
  onToggle,
  onPlay,
  playingIndex = -1,
}) {
  const visible = useMemo(() => {
    const query = search.toLowerCase();
    const filtering = Boolean(speakerFilter || query);
    const result = [];
    // Markers belong to the timeline; they are hidden while filtering.
    for (const item of withInterruptions(
      rows,
      filtering ? [] : interruptions,
    )) {
      if (item.type === "gap") {
        result.push({
          ...item,
          key: `gap-${item.gap.start_ms}-${item.gap.source}-${item.gap.cause}`,
        });
        continue;
      }
      const { row, index } = item;
      if (
        (!speakerFilter || rowSpeakers(row).includes(speakerFilter)) &&
        `${row.speaker} ${row.text}`.toLowerCase().includes(query)
      )
        result.push({ ...item, key: `${row.start_ms}-${index}` });
    }
    return result;
  }, [rows, speakerFilter, search, interruptions]);
  const keys = useMemo(() => visible.map((item) => item.key), [visible]);
  const list = useVirtualList({ keys, estimate: 140 });
  const items = list.indexes.map((position) => {
    const item = visible[position];
    const content =
      item.type === "gap" ? (
        <InterruptionMarker gap={item.gap} />
      ) : (
        <TranscriptRow
          row={item.row}
          index={item.index}
          tone={speakerTone(item.row.speaker_labels?.[0] || item.row.speaker)}
          selectable={selectable}
          selected={Boolean(selected?.has(item.index))}
          onToggle={onToggle}
          onPlay={onPlay}
          playing={item.index === playingIndex}
        />
      );
    return list.enabled ? (
      <div
        className="virtual-row"
        key={item.key}
        data-virtual-key={item.key}
        ref={list.measure(item.key)}
      >
        {content}
      </div>
    ) : (
      <Fragment key={item.key}>{content}</Fragment>
    );
  });
  if (!list.enabled) return items;
  return (
    <div ref={list.container}>
      <div style={{ height: list.before }} aria-hidden="true" />
      {items}
      <div style={{ height: list.after }} aria-hidden="true" />
    </div>
  );
}
