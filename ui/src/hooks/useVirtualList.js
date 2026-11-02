// Renders only the rows near the visible part of a long list.
//
// The list scrolls with its nearest scrolling ancestor, so it can sit inside a
// normal page. Row heights are measured as rows render; unmeasured rows use
// `estimate`. Short lists render every row.
import { useCallback, useEffect, useRef, useState } from "react";

const OVERSCAN_PX = 900;

function scrollParentOf(element) {
  for (let node = element?.parentElement; node; node = node.parentElement) {
    const { overflowY } = getComputedStyle(node);
    if (overflowY === "auto" || overflowY === "scroll") return node;
  }
  return document.scrollingElement;
}

function firstAtOrAfter(starts, heights, top) {
  let low = 0;
  let high = starts.length;
  while (low < high) {
    const middle = (low + high) >> 1;
    if (starts[middle] + heights[middle] < top) low = middle + 1;
    else high = middle;
  }
  return low;
}

export function useVirtualList({ keys, estimate, threshold = 60 }) {
  const enabled = keys.length > threshold;
  const container = useRef(null);
  const measured = useRef(new Map());
  const observer = useRef(null);
  const elements = useRef(new Map());
  const [viewport, setViewport] = useState({ top: 0, bottom: 0 });
  const [version, setVersion] = useState(0);

  // Positions from measured heights. Cheap compared with rendering rows.
  const heights = keys.map((key) => measured.current.get(key) ?? estimate);
  const starts = new Array(keys.length);
  let total = 0;
  for (let index = 0; index < keys.length; index++) {
    starts[index] = total;
    total += heights[index];
  }

  useEffect(() => {
    if (!enabled) return;
    const parent = scrollParentOf(container.current);
    let frame = 0;
    const update = () => {
      frame = 0;
      const list = container.current;
      if (!list || !parent) return;
      const top =
        parent.getBoundingClientRect().top - list.getBoundingClientRect().top;
      const bottom = top + parent.clientHeight;
      setViewport((previous) =>
        Math.abs(previous.top - top) < 4 &&
        Math.abs(previous.bottom - bottom) < 4
          ? previous
          : { top, bottom },
      );
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    update();
    parent?.addEventListener("scroll", schedule, { passive: true });
    const resize = new ResizeObserver(schedule);
    if (parent) resize.observe(parent);
    if (container.current) resize.observe(container.current);
    return () => {
      cancelAnimationFrame(frame);
      parent?.removeEventListener("scroll", schedule);
      resize.disconnect();
    };
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;
    let frame = 0;
    observer.current = new ResizeObserver((entries) => {
      let changed = false;
      for (const entry of entries) {
        const key = entry.target.dataset.virtualKey;
        const height =
          entry.borderBoxSize?.[0]?.blockSize ?? entry.target.offsetHeight;
        if (
          key !== undefined &&
          height > 0 &&
          measured.current.get(key) !== height
        ) {
          measured.current.set(key, height);
          changed = true;
        }
      }
      if (changed && !frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          setVersion((value) => value + 1);
        });
    });
    for (const element of elements.current.values())
      observer.current.observe(element);
    return () => {
      cancelAnimationFrame(frame);
      observer.current?.disconnect();
      observer.current = null;
    };
  }, [enabled]);

  // Forget heights of rows that no longer exist.
  useEffect(() => {
    if (measured.current.size <= keys.length * 2) return;
    const current = new Set(keys);
    for (const key of measured.current.keys())
      if (!current.has(key)) measured.current.delete(key);
  }, [keys]);

  // One stable callback ref per row key, so rendering does not re-observe.
  const callbacks = useRef(new Map());
  const measure = useCallback((key) => {
    let callback = callbacks.current.get(key);
    if (!callback) {
      callback = (element) => {
        const previous = elements.current.get(key);
        if (previous && previous !== element) {
          observer.current?.unobserve(previous);
          elements.current.delete(key);
        }
        if (element) {
          elements.current.set(key, element);
          observer.current?.observe(element);
        } else {
          callbacks.current.delete(key);
        }
      };
      callbacks.current.set(key, callback);
    }
    return callback;
  }, []);

  if (!enabled)
    return {
      enabled,
      container,
      measure,
      indexes: keys.map((_, index) => index),
      before: 0,
      after: 0,
      version,
    };

  const first = firstAtOrAfter(starts, heights, viewport.top - OVERSCAN_PX);
  const indexes = [];
  for (
    let index = first;
    index < keys.length && starts[index] <= viewport.bottom + OVERSCAN_PX;
    index++
  )
    indexes.push(index);
  const last = indexes.at(-1) ?? first - 1;
  return {
    enabled,
    container,
    measure,
    indexes,
    before: starts[first] ?? total,
    after: Math.max(0, total - (starts[last] ?? 0) - (heights[last] ?? 0)),
    version,
  };
}
