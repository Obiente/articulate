// Resolves Discord participant picture identifiers to displayable URLs.
//
// The controller serves cached PNG bytes for `bytes://discord-avatar/...`
// identifiers and answers with nothing until a picture has been fetched, so
// unresolved identifiers are retried a few times with growing delays. Nothing
// else is ever requested: an identifier that does not have that form is
// refused here before it reaches the bridge.

export const AVATAR_PATTERN =
  /^bytes:\/\/discord-avatar\/\d{1,20}\/(a_)?[0-9a-f]{32}\.png$/u;

export const isAvatarUri = (uri) =>
  typeof uri === "string" && AVATAR_PATTERN.test(uri);

export const RETRY_DELAYS_MS = [1500, 3000, 6000, 12000];

const defaultToUrl = (bytes) =>
  URL.createObjectURL(new Blob([bytes], { type: "image/png" }));

// `load(uri)` resolves to PNG bytes or null. `toUrl(bytes)` turns them into a
// URL the image element can show; `wait(ms)` is replaceable for tests.
export function createAvatarCache({
  load,
  toUrl = defaultToUrl,
  wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  retries = RETRY_DELAYS_MS,
} = {}) {
  const urls = new Map();
  const pending = new Map();
  const listeners = new Map();

  const notify = (uri) => {
    for (const listener of listeners.get(uri) || []) listener(urls.get(uri));
  };

  async function fetchWithRetries(uri) {
    for (let attempt = 0; ; attempt += 1) {
      let bytes = null;
      try {
        bytes = await load(uri);
      } catch {
        bytes = null;
      }
      if (bytes?.length) return toUrl(bytes);
      if (attempt >= retries.length) return null;
      await wait(retries[attempt]);
    }
  }

  return {
    // A URL when the picture is ready, null when it is unavailable, or
    // undefined while it is still being resolved.
    peek(uri) {
      return urls.get(uri);
    },
    resolve(uri) {
      if (!isAvatarUri(uri)) return Promise.resolve(null);
      if (urls.has(uri)) return Promise.resolve(urls.get(uri));
      if (!pending.has(uri)) {
        const promise = fetchWithRetries(uri).then((url) => {
          urls.set(uri, url);
          pending.delete(uri);
          notify(uri);
          return url;
        });
        pending.set(uri, promise);
      }
      return pending.get(uri);
    },
    // Observe one identifier; the listener receives the final URL or null.
    subscribe(uri, listener) {
      if (!listeners.has(uri)) listeners.set(uri, new Set());
      listeners.get(uri).add(listener);
      return () => listeners.get(uri)?.delete(listener);
    },
    // Lets a disabled or unavailable picture be tried again later.
    forget(uri) {
      urls.delete(uri);
    },
    size: () => urls.size,
  };
}

// Up to two initials for the fallback badge: "Casey Lane" → "CL".
export function participantInitials(name) {
  const words = String(name || "")
    .trim()
    .split(/\s+/u)
    .filter(Boolean);
  if (!words.length) return "?";
  const letters = words.slice(0, 2).map((word) => [...word][0]);
  return letters.join("").toLocaleUpperCase();
}
