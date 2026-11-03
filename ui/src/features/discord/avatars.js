// Resolves Discord participant picture identifiers to displayable URLs.
//
// The controller serves cached PNG bytes for `bytes://discord-avatar/...`
// identifiers and answers with nothing until a picture has been fetched, so
// unresolved identifiers are retried a few times with growing delays. Nothing
// else is ever requested: an identifier that does not have that form is
// refused here before it reaches the bridge.
//
// A picture that is still unavailable after those retries is remembered only
// briefly, so a slow fetch does not leave a participant blank for the rest of
// the session. Object URLs are revoked once nothing shows them any more.

export const AVATAR_PATTERN =
  /^bytes:\/\/discord-avatar\/\d{1,20}\/(a_)?[0-9a-f]{32}\.png$/u;

export const isAvatarUri = (uri) =>
  typeof uri === "string" && AVATAR_PATTERN.test(uri);

export const RETRY_DELAYS_MS = [1500, 3000, 6000, 12000];

// How long an unavailable result is trusted before the picture is tried again.
export const UNAVAILABLE_TTL_MS = 30_000;

const defaultToUrl = (bytes) =>
  URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
const defaultRevoke = (url) => URL.revokeObjectURL(url);

// `load(uri)` resolves to PNG bytes or null. `toUrl(bytes)` turns them into a
// URL the image element can show and `revoke(url)` frees it again; `wait(ms)`
// and `now()` are replaceable for tests.
export function createAvatarCache({
  load,
  toUrl = defaultToUrl,
  revoke = defaultRevoke,
  wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  retries = RETRY_DELAYS_MS,
  now = () => Date.now(),
  unavailableTtl = UNAVAILABLE_TTL_MS,
} = {}) {
  // uri → { url, expires }; a null url expires, a real one does not.
  const entries = new Map();
  const pending = new Map();
  const listeners = new Map();
  // URLs no longer cached but possibly still on screen, revoked once the
  // identifier has no subscriber left or a replacement has been announced.
  const retired = new Map();

  const subscribers = (uri) => listeners.get(uri)?.size || 0;

  const release = (uri) => {
    const urls = retired.get(uri);
    if (!urls) return;
    retired.delete(uri);
    for (const url of urls) revoke(url);
  };

  const retire = (uri, url) => {
    if (!url) return;
    if (!retired.has(uri)) retired.set(uri, new Set());
    retired.get(uri).add(url);
    if (!subscribers(uri)) release(uri);
  };

  // The cached URL, null while an unavailable result is still trusted, or
  // undefined when nothing usable is known.
  const current = (uri) => {
    const entry = entries.get(uri);
    if (!entry) return undefined;
    if (entry.url === null && entry.expires <= now()) {
      entries.delete(uri);
      return undefined;
    }
    return entry.url;
  };

  const notify = (uri) => {
    for (const listener of listeners.get(uri) || []) listener(current(uri));
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

  const forget = (uri) => {
    const entry = entries.get(uri);
    entries.delete(uri);
    retire(uri, entry?.url);
  };

  return {
    // A URL when the picture is ready, null when it is unavailable, or
    // undefined while it is still being resolved.
    peek(uri) {
      return current(uri);
    },
    resolve(uri) {
      if (!isAvatarUri(uri)) return Promise.resolve(null);
      const known = current(uri);
      if (known !== undefined) return Promise.resolve(known);
      if (!pending.has(uri)) {
        const promise = fetchWithRetries(uri).then((url) => {
          // Replacing an entry retires its old URL; subscribers hear about
          // the new one first, so the old picture is never pulled from under
          // an element that has not re-rendered yet.
          const previous = entries.get(uri);
          if (previous?.url && previous.url !== url) retire(uri, previous.url);
          entries.set(uri, {
            url,
            expires: url === null ? now() + unavailableTtl : Infinity,
          });
          pending.delete(uri);
          notify(uri);
          release(uri);
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
      return () => {
        const set = listeners.get(uri);
        if (!set) return;
        set.delete(listener);
        if (!set.size) {
          listeners.delete(uri);
          release(uri);
        }
      };
    },
    // Lets a disabled, replaced or unavailable picture be tried again later.
    // Its URL is revoked as soon as nothing displays it any more.
    forget,
    // Forgets every picture, for example when pictures are turned off.
    clear() {
      for (const uri of [...entries.keys()]) forget(uri);
    },
    size: () => entries.size,
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
