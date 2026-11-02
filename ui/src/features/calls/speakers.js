// Speaker identity and colors for transcript rows.
import { formatTime } from "../../lib/format.js";

export const speakerColors = [
  "mint",
  "blue",
  "grape",
  "orange",
  "pink",
  "teal",
  "yellow",
  "indigo",
];

export const rowSpeakers = (row) =>
  row.speaker_labels?.length ? row.speaker_labels : [row.speaker];

export function speakersOf(rows) {
  return [...new Set(rows.flatMap(rowSpeakers).filter(Boolean))];
}

export function speakerToneFor(speakers) {
  return (speaker) =>
    speakerColors[
      Math.max(0, speakers.indexOf(speaker)) % speakerColors.length
    ];
}

// The plain-text transcript used for copy and export.
export function transcriptText(rows, fallback = "") {
  return rows.length
    ? rows
        .map((r) => `[${formatTime(r.start_ms)}] ${r.speaker}: ${r.text}`)
        .join("\n\n")
    : fallback;
}
