import { memo, useMemo } from "react";
import { rowSpeakers } from "./speakers.js";

// Speaker turns on a shared time axis. Selecting a speaker filters the
// transcript to their turns.
export const SpeakerTimeline = memo(function SpeakerTimeline({
  rows,
  speakers,
  speakerTone,
  live,
  seconds,
  speakerFilter,
  onFilter,
}) {
  const tracks = useMemo(() => {
    const end = rows.reduce(
      (latest, row) => Math.max(latest, row.end_ms || 0),
      Math.max(seconds * 1000 || 0, 1),
    );
    const start = live ? Math.max(0, end - 90000) : 0;
    const range = Math.max(1, end - start);
    const bars = new Map(speakers.map((speaker) => [speaker, []]));
    rows.forEach((row, index) => {
      const from = Math.max(start, row.start_ms || 0);
      const to = Math.min(end, row.end_ms || from);
      if (to <= from) return;
      for (const speaker of rowSpeakers(row))
        bars.get(speaker)?.push({
          key: `${row.start_ms}-${index}`,
          left: `${((from - start) / range) * 100}%`,
          width: `${Math.max(0.6, ((to - from) / range) * 100)}%`,
        });
    });
    return bars;
  }, [rows, speakers, live, seconds]);
  return (
    <section className="speaker-timeline" aria-label="Speaker turns">
      <div className="speaker-timeline-heading">
        <strong>Speaker turns</strong>
        <span>{live ? "Recent 90 seconds" : "Conversation overview"}</span>
      </div>
      {speakers.map((speaker) => (
        <button
          className={`speaker-track ${speakerFilter === speaker ? "selected" : ""}`}
          key={speaker}
          type="button"
          onClick={() => onFilter(speakerFilter === speaker ? null : speaker)}
          aria-pressed={speakerFilter === speaker}
          title={`Show ${speaker}'s turns`}
        >
          <span
            className={`speaker-track-name speaker-tone-${speakerTone(speaker)}`}
          >
            {speaker}
          </span>
          <span className="speaker-track-line">
            {tracks.get(speaker).map((bar) => (
              <span
                className={`speaker-track-bar speaker-tone-bg-${speakerTone(speaker)}`}
                key={bar.key}
                style={{ left: bar.left, width: bar.width }}
              />
            ))}
          </span>
        </button>
      ))}
    </section>
  );
});
