// Speaker identity, colors and transcript helpers for conversation rows.
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

// What a speaker key stands for. Keys are opaque; only their prefix is read.
export function speakerKind(key) {
  if (key === "you") return "you";
  if (key === "other" || key === "uncertain") return "other";
  if (/^s[1-8]$/.test(key)) return "voice";
  return "person";
}

// Every distinct speaker in order of first appearance, with their turn count.
// Rows from older snapshots without keys use their label as the key.
export function rosterOf(rows) {
  const roster = new Map();
  for (const row of rows) {
    const labels = rowSpeakers(row);
    labels.forEach((label, index) => {
      if (!label) return;
      const key = row.speaker_keys?.[index] ?? `label:${label}`;
      const existing = roster.get(key);
      if (existing) existing.turns += 1;
      else
        roster.set(key, {
          key,
          label,
          kind: row.speaker_keys ? speakerKind(key) : "other",
          turns: 1,
        });
    });
  }
  return [...roster.values()].map((speaker) => ({
    ...speaker,
    renamable: speaker.kind === "voice" || speaker.kind === "person",
  }));
}

// Rows and interruption markers in time order. Markers come before a row
// that starts at or after them, like the native exports.
export function withInterruptions(rows, interruptions = []) {
  if (!interruptions.length)
    return rows.map((row, index) => ({ type: "row", row, index }));
  const gaps = [...interruptions].sort((a, b) => a.start_ms - b.start_ms);
  const items = [];
  let next = 0;
  rows.forEach((row, index) => {
    while (next < gaps.length && gaps[next].start_ms <= row.start_ms)
      items.push({ type: "gap", gap: gaps[next++] });
    items.push({ type: "row", row, index });
  });
  while (next < gaps.length) items.push({ type: "gap", gap: gaps[next++] });
  return items;
}

// The plain-text transcript used for copy and export.
export function transcriptText(rows, fallback = "", interruptions = []) {
  if (!rows.length) return fallback;
  return withInterruptions(rows, interruptions)
    .map((item) =>
      item.type === "gap"
        ? `[${item.gap.message}]`
        : `[${formatTime(item.row.start_ms)}] ${item.row.speaker}: ${item.row.text}`,
    )
    .join("\n\n");
}

// The row being heard during playback: the last one that started.
export function playingRowIndex(rows, positionMs) {
  if (!Number.isFinite(positionMs)) return -1;
  let found = -1;
  for (let index = 0; index < rows.length; index += 1) {
    if (rows[index].start_ms > positionMs) break;
    found = index;
  }
  return found;
}

// Known names to offer when renaming: people first, without names already
// used by another speaker in this transcript.
export function nameSuggestions(people = [], roster = [], current = "") {
  const used = new Set(
    roster
      .filter((speaker) => speaker.label !== current)
      .map((speaker) => speaker.label.toLowerCase()),
  );
  const seen = new Set();
  const names = [];
  for (const person of people) {
    const lower = person.name.toLowerCase();
    if (used.has(lower) || seen.has(lower)) continue;
    seen.add(lower);
    names.push(person.name);
  }
  return names;
}

export const formatBytes = (bytes) =>
  bytes >= 1024 * 1024
    ? `${(bytes / 1024 / 1024).toFixed(1)} MB`
    : `${Math.max(1, Math.round(bytes / 1024))} KB`;
