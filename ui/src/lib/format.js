// Shared display formatting.
const dateFormat = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});
const clockFormat = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
});

export const formatDate = (ms) => dateFormat.format(ms);
export const formatClock = (ms) => clockFormat.format(ms);
export const formatDuration = (ms) =>
  ms ? `${Math.max(1, Math.round(ms / 60000))} min` : "";
export const formatTime = (ms) =>
  `${Math.floor(ms / 60000)
    .toString()
    .padStart(2, "0")}:${Math.floor((ms / 1000) % 60)
    .toString()
    .padStart(2, "0")}`;
export const countWords = (text) =>
  text.trim().split(/\s+/).filter(Boolean).length;
export const formatCount = (value) => Number(value || 0).toLocaleString();

export const shortcutKeys = (hotkey) =>
  [
    hotkey?.ctrl && "Ctrl",
    hotkey?.alt && "Alt",
    hotkey?.shift && "Shift",
    hotkey?.win && "Win",
    hotkey?.key,
  ].filter(Boolean);
export const shortcutLabel = (hotkey) => shortcutKeys(hotkey).join(" + ");

// The dictation shortcut, with Space as the default key.
export const dictationShortcut = (hotkey) =>
  shortcutKeys({ ...hotkey, key: hotkey?.key || "Space" }).join(" + ");

const languageNames = new Intl.DisplayNames(["en"], { type: "language" });
export function languageLabel(code) {
  try {
    return languageNames.of(code) || code;
  } catch {
    // Show the model's locale code when it has no display name.
    return code;
  }
}
