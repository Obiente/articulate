// Items that need the user's attention: isolated damaged files, plus records
// that could not be read this time or were saved by a newer version. Damaged
// records are already counted as isolated files.
export function attentionCount(recovery) {
  if (!recovery) return 0;
  const others = (recovery.warnings || []).filter(
    (warning) => warning.kind !== "damaged",
  ).length;
  return (recovery.quarantined || 0) + others;
}
