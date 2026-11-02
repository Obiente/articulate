import { memo, useMemo } from "react";
import { Avatar, Badge, Group } from "@mantine/core";
import { formatTime } from "../../lib/format.js";
import { useVirtualList } from "../../hooks/useVirtualList.js";
import { rowSpeakers } from "./speakers.js";

const TranscriptRow = memo(function TranscriptRow({ row, tone }) {
  return (
    <article
      className={`transcript-row ${row.provisional ? "provisional" : ""}`}
    >
      <Avatar radius="xl" size={32} color={tone}>
        {row.speaker?.slice(0, 1) || "?"}
      </Avatar>
      <div>
        <div className="speaker-line">
          <strong>{row.speaker}</strong>
          <time>{formatTime(row.start_ms)}</time>
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

// Transcript rows, filtered by speaker and search text. Long transcripts
// render only the rows near the visible area.
export function TranscriptRows({ rows, speakerFilter, search, speakerTone }) {
  const visible = useMemo(() => {
    const query = search.toLowerCase();
    const result = [];
    rows.forEach((row, index) => {
      if (
        (!speakerFilter || rowSpeakers(row).includes(speakerFilter)) &&
        `${row.speaker} ${row.text}`.toLowerCase().includes(query)
      )
        result.push({ row, key: `${row.start_ms}-${index}` });
    });
    return result;
  }, [rows, speakerFilter, search]);
  const keys = useMemo(() => visible.map((item) => item.key), [visible]);
  const list = useVirtualList({ keys, estimate: 140 });
  const items = list.indexes.map((index) => {
    const { row, key } = visible[index];
    const tone = speakerTone(row.speaker_labels?.[0] || row.speaker);
    return list.enabled ? (
      <div
        className="virtual-row"
        key={key}
        data-virtual-key={key}
        ref={list.measure(key)}
      >
        <TranscriptRow row={row} tone={tone} />
      </div>
    ) : (
      <TranscriptRow key={key} row={row} tone={tone} />
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
