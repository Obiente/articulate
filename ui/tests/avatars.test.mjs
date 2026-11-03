import assert from "node:assert/strict";
import test from "node:test";
import {
  createAvatarCache,
  isAvatarUri,
  participantInitials,
} from "../src/features/discord/avatars.js";

const valid =
  "bytes://discord-avatar/123456/0123456789abcdef0123456789abcdef.png";

test("only identifier-shaped avatar URIs are accepted", () => {
  assert.ok(isAvatarUri(valid));
  assert.ok(
    isAvatarUri(
      "bytes://discord-avatar/123456/a_0123456789abcdef0123456789abcdef.png",
    ),
  );
  for (const uri of [
    "",
    null,
    "https://cdn.discordapp.com/avatars/1/0123456789abcdef0123456789abcdef.png",
    "bytes://discord-avatar/../x/0123456789abcdef0123456789abcdef.png",
    "bytes://discord-avatar/123456/0123456789abcdef0123456789abcdef.svg",
    "bytes://discord-avatar/123456/0123456789abcdef0123456789abcdef.png?x=1",
    "bytes://discord-avatar/123456789012345678901/0123456789abcdef0123456789abcdef.png",
    "bytes://discord-avatar/123456/0123456789ABCDEF0123456789abcdef.png",
  ]) {
    assert.equal(isAvatarUri(uri), false, String(uri));
  }
});

test("initials use up to two words and never read as empty", () => {
  assert.equal(participantInitials("Casey Lane"), "CL");
  assert.equal(participantInitials("  jordan  "), "J");
  assert.equal(participantInitials("Ana Maria Costa"), "AM");
  assert.equal(participantInitials("émile"), "É");
  assert.equal(participantInitials(""), "?");
  assert.equal(participantInitials(undefined), "?");
});

test("the cache retries an uncached picture, then remembers the result", async () => {
  const waits = [];
  let answers = [null, null, new Uint8Array([1, 2, 3])];
  let loads = 0;
  const cache = createAvatarCache({
    load: async () => {
      loads += 1;
      return answers.shift() ?? null;
    },
    toUrl: (bytes) => `blob:${bytes.length}`,
    wait: async (ms) => {
      waits.push(ms);
    },
    retries: [10, 20, 30],
  });
  assert.equal(cache.peek(valid), undefined);
  const seen = [];
  cache.subscribe(valid, (url) => seen.push(url));
  const first = cache.resolve(valid);
  const second = cache.resolve(valid);
  assert.equal(first, second, "concurrent lookups share one request");
  assert.equal(await first, "blob:3");
  assert.deepEqual(waits, [10, 20]);
  assert.equal(loads, 3);
  assert.deepEqual(seen, ["blob:3"]);
  assert.equal(cache.peek(valid), "blob:3");
  assert.equal(await cache.resolve(valid), "blob:3");
  assert.equal(loads, 3, "a resolved picture is not fetched again");
});

test("the cache gives up after the retry budget and never loads bad URIs", async () => {
  let loads = 0;
  const cache = createAvatarCache({
    load: async () => {
      loads += 1;
      throw new Error("offline");
    },
    toUrl: () => "unused",
    wait: async () => {},
    retries: [1, 1],
  });
  assert.equal(await cache.resolve(valid), null);
  assert.equal(loads, 3);
  assert.equal(cache.peek(valid), null);
  cache.forget(valid);
  assert.equal(cache.peek(valid), undefined);
  assert.equal(await cache.resolve("https://example.invalid/avatar.png"), null);
  assert.equal(loads, 3, "non-identifier URIs never reach the bridge");
});

test("an unavailable picture is remembered only briefly, then tried again", async () => {
  let clock = 0;
  const answers = [null, new Uint8Array([9])];
  let loads = 0;
  const cache = createAvatarCache({
    load: async () => {
      loads += 1;
      return answers.shift() ?? null;
    },
    toUrl: (bytes) => `blob:${bytes.length}`,
    wait: async () => {},
    retries: [],
    now: () => clock,
    unavailableTtl: 1000,
  });
  const seen = [];
  cache.subscribe(valid, (url) => seen.push(url));
  assert.equal(await cache.resolve(valid), null);
  assert.equal(loads, 1);
  assert.equal(cache.peek(valid), null, "trusted while fresh");
  assert.equal(await cache.resolve(valid), null);
  assert.equal(loads, 1, "not fetched again while the result is fresh");
  clock = 1000;
  assert.equal(cache.peek(valid), undefined, "expired, so unknown again");
  assert.equal(cache.size(), 0);
  assert.equal(await cache.resolve(valid), "blob:1");
  assert.equal(loads, 2);
  assert.deepEqual(seen, [null, "blob:1"]);
  assert.equal(cache.peek(valid), "blob:1");
  clock = 1_000_000;
  assert.equal(cache.peek(valid), "blob:1", "a real picture never expires");
});

test("object URLs are revoked once nothing shows them", async () => {
  const revoked = [];
  let serial = 0;
  const cache = createAvatarCache({
    load: async () => new Uint8Array([1]),
    toUrl: () => `blob:${(serial += 1)}`,
    revoke: (url) => revoked.push(url),
    wait: async () => {},
    retries: [],
  });
  const other = "bytes://discord-avatar/7/0123456789abcdef0123456789abcdef.png";
  assert.equal(await cache.resolve(valid), "blob:1");
  cache.forget(valid);
  assert.deepEqual(revoked, ["blob:1"], "nothing shows it, so it is freed at once");
  // While a subscriber still shows a picture, forgetting defers revocation
  // until the subscriber lets go.
  const unsubscribe = cache.subscribe(valid, () => {});
  assert.equal(await cache.resolve(valid), "blob:2");
  cache.forget(valid);
  assert.deepEqual(revoked, ["blob:1"]);
  unsubscribe();
  assert.deepEqual(revoked, ["blob:1", "blob:2"]);
  // A replacement is announced to subscribers before the old URL is freed.
  const seen = [];
  const stop = cache.subscribe(valid, (url) => seen.push(url));
  assert.equal(await cache.resolve(valid), "blob:3");
  cache.forget(valid);
  assert.equal(await cache.resolve(valid), "blob:4");
  assert.deepEqual(seen, ["blob:3", "blob:4"]);
  assert.deepEqual(revoked, ["blob:1", "blob:2", "blob:3"]);
  stop();
  assert.deepEqual(revoked, ["blob:1", "blob:2", "blob:3"]);
  // Clearing, for example when pictures are turned off, frees everything.
  assert.equal(await cache.resolve(other), "blob:5");
  assert.equal(cache.size(), 2);
  cache.clear();
  assert.equal(cache.size(), 0);
  assert.deepEqual(revoked, ["blob:1", "blob:2", "blob:3", "blob:4", "blob:5"]);
  assert.equal(await cache.resolve(valid), "blob:6", "a cleared picture loads again");
});
