// Run with: node --test tests/browser/avatar-model.test.js
//
// The model is fetched from a third-party host and checked against a pinned
// hash before anything renders it, and every interesting branch of that lives
// in web/avatar/model.js: a cache hit, a poisoned cache, a hash miss, a browser
// that will not store 11 MB. The browser check drives exactly one of those, the
// happy path over a real network, so this file exists to drive the rest.
//
// Real bytes and a real SHA-256 throughout. Node has WebCrypto and `Response`,
// so nothing here fakes a primitive the code relies on. Only the cache and the
// network are stubs, because those are the two things a test cannot have. The
// pin is passed in rather than faked, which is why `modelBytes` takes it.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  MODEL_CACHE,
  MODEL_SHA256,
  MODEL_URL,
  evictSuperseded,
  loadModelBytes,
  modelBytes,
  openModelCache,
} from "../../web/avatar/model.js";
import { failFetchWith, read } from "./source.js";

const MODEL = new TextEncoder().encode("stand-in for 11 MB of Seed-san").buffer;
const IMPOSTOR = new TextEncoder().encode("something else entirely").buffer;

async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

const PIN = await sha256Hex(MODEL);
// A previous model's cache: the same prefix, a different full-length hash.
const SUPERSEDED = `codetrial-avatar-${"a".repeat(64)}`;

/// A Cache API stub narrow enough to be obviously correct, counting the calls
/// the assertions care about. `put` stores what it was handed rather than the
/// Response object, so a hit reads back the same bytes a browser would.
function fakeCache(seed) {
  const store = new Map();
  if (seed) store.set("model-url", seed);
  return {
    puts: 0,
    deletes: 0,
    async match(url) {
      const bytes = store.get(url);
      return bytes ? new Response(bytes) : undefined;
    },
    async put(url, response) {
      this.puts += 1;
      store.set(url, await response.arrayBuffer());
    },
    async delete(url) {
      this.deletes += 1;
      return store.delete(url);
    },
  };
}

/// Wraps the shared fetch swap around one call, so a failing assertion cannot
/// leak a stub into the next test.
async function withFetch(handler, body) {
  const restore = failFetchWith(handler);
  try {
    return await body();
  } finally {
    restore();
  }
}

/// A fetch stub that serves the model and counts how often it was asked.
function counted() {
  const state = { calls: 0 };
  state.fetch = async () => {
    state.calls += 1;
    return new Response(MODEL);
  };
  return state;
}

test("the pin in the module is the one the license record documents", () => {
  // The model is not in the tree any more, so these two are the only places
  // that say which bytes are Jim, and they have to agree.
  assert.match(read("web/vendor/avatar/LICENSE-jim-vrm.txt"), new RegExp(MODEL_SHA256));
});

test("the CSP names the origin the model is actually fetched from", () => {
  // The third copy of this fact, and the only one a language boundary forces.
  // Drift here is invisible in the worst way: the CSP blocks the download and
  // the avatar degrades to the neutral panel, which is what every other
  // failure looks like too.
  const origin = read("src/web/policy.rs").match(/AVATAR_MODEL_ORIGIN: &str = "([^"]+)"/)?.[1];
  assert.ok(origin, "src/web/policy.rs must keep naming the avatar model origin");
  assert.ok(
    MODEL_URL.startsWith(`${origin}/`),
    `MODEL_URL ${MODEL_URL} is not under the CSP-permitted origin ${origin}`,
  );
});

test("a cold load fetches, verifies, stores, and returns the bytes", async () => {
  const cache = fakeCache();
  const network = counted();
  const bytes = await withFetch(network.fetch, () => modelBytes("model-url", cache, PIN));

  assert.equal(await sha256Hex(bytes), PIN);
  assert.equal(network.calls, 1);
  assert.equal(cache.puts, 1, "a verified download is worth keeping");
});

test("a warm load reads the cache and never touches the network", async () => {
  const cache = fakeCache(MODEL);
  const network = counted();
  const bytes = await withFetch(network.fetch, () => modelBytes("model-url", cache, PIN));

  assert.equal(await sha256Hex(bytes), PIN);
  assert.equal(network.calls, 0, "the whole point of the cache is this");
  assert.equal(cache.puts, 0);
});

test("a cached entry that misses the pin is dropped and re-fetched, not trusted", async () => {
  const cache = fakeCache(IMPOSTOR);
  const network = counted();
  const bytes = await withFetch(network.fetch, () => modelBytes("model-url", cache, PIN));

  assert.equal(await sha256Hex(bytes), PIN, "the load recovers rather than failing");
  assert.equal(cache.deletes, 1, "a truncated or replaced entry must not survive");
  assert.equal(network.calls, 1);
  assert.equal(cache.puts, 1, "and the good bytes replace it");
});

for (const [why, cache] of [
  ["a storage backend that went away mid-session", {
    async match() { throw new DOMException("gone", "InvalidStateError"); },
    async put() {},
  }],
  ["Firefox private browsing, or an origin already at its quota", {
    async match() { return undefined; },
    async put() { throw new DOMException("quota", "QuotaExceededError"); },
  }],
  ["a browser with no Cache API at all", null],
  // Not hypothetical bookkeeping: the store is fired and not awaited, so a
  // `put` that throws instead of rejecting would escape a bare `.catch` on the
  // returned promise and take the load down with it.
  ["a put that throws synchronously rather than rejecting", {
    async match() { return undefined; },
    put() { throw new DOMException("bad request", "TypeError"); },
  }],
]) {
  test(`losing the cache costs a download, not the avatar: ${why}`, async () => {
    const bytes = await withFetch(async () => new Response(MODEL), () =>
      modelBytes("model-url", cache, PIN));
    assert.equal(await sha256Hex(bytes), PIN);
  });
}

test("bytes that miss the pin are never returned", async () => {
  const cache = fakeCache();
  await withFetch(async () => new Response(IMPOSTOR), async () => {
    await assert.rejects(
      () => modelBytes("model-url", cache, PIN),
      /did not match its pinned SHA-256/,
      "this is the check the whole design rests on",
    );
  });
  assert.equal(cache.puts, 0, "and unverified bytes are never cached for next time");
});

test("a download that fails says so instead of loading the error page", async () => {
  await withFetch(async () => new Response("not found", { status: 404 }), async () => {
    await assert.rejects(() => modelBytes("model-url", null, PIN), /answered 404/);
  });
});

test("an unopenable cache degrades to no cache instead of failing the load", async () => {
  assert.equal(await openModelCache(undefined), null, "a browser with no Cache API is not an error");
  assert.equal(
    await openModelCache({
      async open() {
        throw new DOMException("private browsing", "SecurityError");
      },
    }),
    null,
  );
});

test("the sweep collects superseded models and spares everything else", async () => {
  const deleted = [];
  // Awaited directly rather than through openModelCache, which fires it and
  // does not wait. Asserting on the side effect after a non-awaited call would
  // pass or fail on scheduling luck.
  await evictSuperseded({
    async keys() {
      return [MODEL_CACHE, SUPERSEDED, "some-other-app", "codetrial-avatar-short"];
    },
    async delete(name) {
      deleted.push(name);
      return true;
    },
  });

  // Only the one that looks like a real model cache. A name with the prefix but
  // a short tail is somebody else's, and is left alone.
  assert.deepEqual(deleted, [SUPERSEDED],
    "11 MB per superseded model, and nothing else ever collects them");
});

test("opening the cache does not wait for the sweep", async () => {
  // The candidate is waiting on the model behind this. A sweep that hangs, or
  // that has an 11 MB delete to do first, must not hold the open.
  const cache = await openModelCache({
    async open(name) {
      return { name };
    },
    keys() {
      return new Promise(() => {});
    },
    async delete() {
      return true;
    },
  });

  assert.equal(cache.name, MODEL_CACHE);
});

test("a context that cannot verify refuses rather than loading unchecked bytes", async () => {
  // An insecure context has no crypto.subtle, and `http://` on a LAN address is
  // exactly how this gets demonstrated to someone.
  const original = Object.getOwnPropertyDescriptor(globalThis, "crypto");
  Object.defineProperty(globalThis, "crypto", { value: {}, configurable: true });
  try {
    await assert.rejects(
      () => loadModelBytes("model-url"),
      /without a secure context/,
      "unverifiable must mean the neutral panel, not 11 MB of unchecked geometry",
    );
  } finally {
    Object.defineProperty(globalThis, "crypto", original);
  }
});
