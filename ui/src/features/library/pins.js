// Pinned items first; otherwise the saved order (most recently changed).
export function pinnedFirst(items) {
  const pinned = [],
    rest = [];
  for (const item of items) (item.pinned ? pinned : rest).push(item);
  return pinned.length ? [...pinned, ...rest] : items;
}
