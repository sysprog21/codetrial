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
  MODEL_MAX_BYTES,
  MODEL_SHA256,
  MODEL_URL,
  evictSuperseded,
  loadModelBytes,
  modelBytes,
  openModelCache,
} from "../../web/avatar/model.js";
import { LOAD_TIMEOUT_MS } from "../../web/avatar/avatar.js";
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

/// A fetch stub that counts how often it was asked, serving the model unless
/// the caller wants some other answer.
function counted(respond = () => new Response(MODEL)) {
  const state = { calls: 0 };
  state.fetch = async () => {
    state.calls += 1;
    return respond();
  };
  return state;
}

/// A response that declares a length and refuses to be read: the shape the
/// header check has to turn away without a transfer. Counts both halves, so a
/// test can say the body was never touched and the origin was never asked
/// twice.
function declares(length) {
  const state = { calls: 0, started: 0, released: 0 };
  state.fetch = async () => {
    state.calls += 1;
    return {
      ok: true,
      status: 200,
      headers: new Headers({ "content-length": length }),
      get body() {
        return {
          cancel() {
            state.released += 1;
          },
          getReader() {
            state.started += 1;
            throw new Error("the body must not be read");
          },
        };
      },
    };
  };
  return state;
}

/// A response that streams `bytes` in fixed-size pieces and then ends. The
/// pieces are views onto one buffer, which is what a real body hands out.
function sends(bytes, chunk) {
  let at = 0;
  return async () =>
    new Response(
      new ReadableStream({
        pull(controller) {
          if (at >= bytes.byteLength) {
            controller.close();
            return;
          }
          controller.enqueue(bytes.subarray(at, at + chunk));
          at += chunk;
        },
      }),
    );
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
      () => loadModelBytes(),
      /without a secure context/,
      "unverifiable must mean the neutral panel, not 11 MB of unchecked geometry",
    );
  } finally {
    Object.defineProperty(globalThis, "crypto", original);
  }
});

// What the fetch refuses, and what it retries. The model comes from a
// third-party host over a link nobody here controls, and every case below is
// something that host can do to a candidate's tab in the middle of an
// interview: send them somewhere else, send more than a model, or fail once.

test("the ceiling is the top of the documented budget and clears the real model", () => {
  const contract = read("docs/avatar-contract.md");
  assert.equal(MODEL_MAX_BYTES, 15 * 1024 * 1024, "binary megabytes, and 15 is the budget's top");
  assert.match(contract, /5 to 15 MB/, "the budget the ceiling is taken from");

  // The pinned model has to fit under its own ceiling, or this whole item
  // breaks the avatar it was written to protect.
  const recorded = contract.match(/\| Size \| ([\d,]+) bytes/)?.[1];
  assert.ok(recorded, "docs/avatar-contract.md must keep recording the model's size");
  assert.ok(
    Number(recorded.replaceAll(",", "")) < MODEL_MAX_BYTES,
    `the pinned model is ${recorded} bytes and must fit under the ${MODEL_MAX_BYTES} ceiling`,
  );

  // The number is written as a commission's budget, so the sentence saying the
  // fetch enforces it is what tells a reader it is now two things. Anchored to
  // that sentence: a bare `/enforce/` is satisfied by the word appearing
  // anywhere in seventy lines, which is a test that cannot fail for its reason.
  assert.match(
    contract,
    /size half of that budget is enforced rather than advisory/,
    "the budget must say the fetch enforces it",
  );
  assert.match(contract, /MODEL_MAX_BYTES/, "and must name the constant that does");
});

test("a redirect is refused rather than followed to another origin", async () => {
  let asked = 0;
  const wanderingOrigin = async (url, init) => {
    asked += 1;
    // What a browser does with `redirect: "error"`: the redirect is not
    // followed, and the fetch rejects instead of returning the other origin's
    // answer. Without the option this stub hands back a perfectly good model.
    if (init?.redirect === "error") throw new TypeError("Failed to fetch");
    return new Response(MODEL);
  };

  await withFetch(wanderingOrigin, async () => {
    await assert.rejects(
      () => modelBytes("model-url", null, PIN),
      /could not be fetched/,
      "CSP permits Compiler Explorer and every LiveKit URL, so it does not pin this",
    );
  });
  // Twice, because a refused redirect reaches the retry as a rejected fetch and
  // nothing distinguishes it from a dropped connection.
  assert.equal(asked, 2);
});

test("a declared length over the ceiling is refused before the body is read", async () => {
  const origin = declares(String(MODEL_MAX_BYTES + 1));
  await withFetch(origin.fetch, async () => {
    await assert.rejects(() => modelBytes("model-url", null, PIN), /over the .* ceiling/);
  });
  assert.equal(origin.started, 0, "the refusal must not wait for a transfer nobody wants");
  assert.equal(origin.calls, 1, "and an origin that has said what it will send is not asked twice");
  // Refusing is not enough on its own: the body is already on its way by the
  // time the header is read, so a refusal that walks away from the stream
  // leaves the connection open and the bytes arriving behind a load that has
  // already failed, which is the transfer the ceiling exists to refuse.
  assert.equal(origin.released, 1, "the refused body must be let go, not left running");
});

test("a body that passes the ceiling while streaming is refused mid-transfer", async () => {
  // No declared length, so the check above cannot see this one coming: the
  // origin simply keeps sending. One megabyte per chunk, reused because nothing
  // ever reads it back.
  const chunk = new Uint8Array(1024 * 1024);
  let sent = 0;
  const endlessBody = async () =>
    new Response(
      new ReadableStream({
        pull(controller) {
          sent += 1;
          controller.enqueue(chunk);
        },
      }),
    );

  await withFetch(endlessBody, async () => {
    await assert.rejects(() => modelBytes("model-url", null, PIN), /sent more than/);
  });
  assert.ok(sent > 0, "the stream was read");
  // Fifteen chunks fit under the ceiling, the sixteenth passes it, and the
  // stream had already pulled one more ahead of the reader. Seventeen, then,
  // and the point of the assertion is that it is bounded at all: `arrayBuffer()`
  // never stops, and this stream never ends.
  assert.ok(
    sent <= MODEL_MAX_BYTES / chunk.byteLength + 2,
    `read ${sent} MB from a body with no end, which is not a ceiling`,
  );
});

test("bytes that miss the pin are never returned, and are tried exactly twice", async () => {
  // The check the whole design rests on, and the one failure worth repeating: a
  // truncated download has already discarded what it had, so a second attempt
  // can answer differently. Twice, and not a loop.
  const cache = fakeCache();
  const network = counted(() => new Response(IMPOSTOR));
  await withFetch(network.fetch, async () => {
    await assert.rejects(
      () => modelBytes("model-url", cache, PIN),
      /did not match its pinned SHA-256/,
    );
  });
  assert.equal(network.calls, 2);
  assert.equal(cache.puts, 0, "and unverified bytes are never cached for next time");
});

test("a 404 is not retried: the origin will answer the same way twice", async () => {
  // A real stream rather than a string body, so the release is observed the way
  // a browser performs it: the underlying source is told when it is cancelled.
  let released = 0;
  const errorPage = () =>
    new Response(
      new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode("not found"));
        },
        cancel() {
          released += 1;
        },
      }),
      { status: 404 },
    );
  const network = counted(errorPage);
  await withFetch(network.fetch, async () => {
    await assert.rejects(() => modelBytes("model-url", null, PIN), /answered 404/);
  });
  assert.equal(network.calls, 1, "retrying a status only doubles the wait before the panel");
  // A refused status is the same leak a refused length is: the page body is
  // already on its way, and a load that walks away from it holds the connection
  // for as long as the origin keeps writing.
  assert.equal(released, 1, "the error page's body is let go rather than left open");
});

test("one transient failure followed by a good response still loads the model", async () => {
  // The reason this item is a P2 rather than a nicety: before the retry, a
  // candidate whose model fetch blipped once sat the whole interview with the
  // neutral panel.
  const cache = fakeCache();
  let calls = 0;
  const bytes = await withFetch(
    async () => {
      calls += 1;
      if (calls === 1) throw new TypeError("NetworkError when attempting to fetch resource.");
      return new Response(MODEL);
    },
    () => modelBytes("model-url", cache, PIN),
  );

  assert.equal(await sha256Hex(bytes), PIN);
  assert.equal(calls, 2);
  assert.equal(cache.puts, 1, "and the recovered bytes are kept like any other download");
});

test("a declared length of exactly the ceiling is not refused", async () => {
  // The boundary the refusal is written on. A `>=` here would refuse a model
  // that weighs precisely the documented budget, which is a size the contract
  // says is allowed. The body is the stand-in rather than 15 MB of anything:
  // what is under test is the header, and the pin proves the read went through.
  const declaresTheCeiling = async () =>
    new Response(MODEL, { headers: { "content-length": String(MODEL_MAX_BYTES) } });

  const bytes = await withFetch(declaresTheCeiling, () => modelBytes("model-url", null, PIN));
  assert.equal(await sha256Hex(bytes), PIN);
});

// Every one of these comes out under the ceiling if the header is read with a
// bare `Number`, and every one of them is a body this must refuse without
// reading. The expected message differs because the reasons do: two of them
// cannot be read as a length at all, and the third reads as a length no browser
// could deliver.
for (const [why, value, refusal] of [
  // `Number("15728641, 15728641")` is `NaN`, and `NaN > ceiling` is false, so a
  // lax read lets the largest declaration in the list through untouched.
  [
    "two values joined, which is what Headers does with a duplicate",
    "15728641, 15728641",
    /unreadable Content-Length/,
  ],
  ["a length that is not digits at all", "about eleven megabytes", /unreadable Content-Length/],
  // `Number` of four hundred digits is `Infinity`. It is a readable length, so
  // it is refused as the enormous one it is rather than as nonsense; what it
  // catches is a comparison guarded on the value being finite first.
  ["a number too large to be a number", "1".repeat(400), /over the .* ceiling/],
]) {
  test(`a Content-Length no browser should act on is refused: ${why}`, async () => {
    const origin = declares(value);
    await withFetch(origin.fetch, async () => {
      await assert.rejects(
        () => modelBytes("model-url", null, PIN),
        refusal,
        "an origin whose declaration cannot be trusted is one to stop at, not to guess about",
      );
    });
    assert.equal(origin.started, 0);
    assert.equal(origin.released, 1, "an unreadable declaration lets the body go too");
    assert.equal(origin.calls, 1, "a declaration is an answer, and asking again gets the same one");
  });
}

test("a body delivered in chunks is reassembled in the order it arrived", async () => {
  // Real streams hand out views onto a shared buffer, so every chunk after the
  // first has a non-zero `byteOffset` and none of them owns its whole buffer.
  // Copying a chunk's buffer rather than the chunk would pass a single-chunk
  // test and corrupt every model that arrives in more than one packet.
  const whole = new Uint8Array(MODEL);
  const cuts = [0, 7, 19, whole.byteLength];
  const pieces = cuts.slice(0, -1).map((from, index) => whole.subarray(from, cuts[index + 1]));
  assert.ok(
    pieces.slice(1).every((piece) => piece.byteOffset > 0),
    "the point of the fixture is the offsets",
  );

  const inPieces = async () =>
    new Response(
      new ReadableStream({
        start(controller) {
          for (const piece of pieces) controller.enqueue(piece);
          controller.close();
        },
      }),
    );

  const bytes = await withFetch(inPieces, () => modelBytes("model-url", null, PIN));
  assert.equal(bytes.byteLength, whole.byteLength);
  assert.equal(await sha256Hex(bytes), PIN);
});

test("a body of exactly the ceiling streams through rather than being refused", async () => {
  // The other side of the boundary above, on the running total rather than on
  // the header, and the one place a full-size body is worth allocating: a `>=`
  // here would refuse a model of exactly the budgeted size after paying for all
  // of it. Zeroes, because only the length is under test.
  const full = new Uint8Array(MODEL_MAX_BYTES);
  const pin = await sha256Hex(full.buffer);

  const bytes = await withFetch(sends(full, 1024 * 1024), () =>
    modelBytes("model-url", null, pin));
  // The length, and not the hash: `modelBytes` returns nothing that missed the
  // pin, so hashing 15 MB again here would cost 20 ms to assert what returning
  // at all already proved.
  assert.equal(bytes.byteLength, MODEL_MAX_BYTES);
});

test("a connection that drops partway through the body is retried like any other", async () => {
  // The failure this file did not have a case for: the fetch resolves, the
  // headers are fine, and the transport dies eight megabytes in. It rejects out
  // of the body read rather than out of `fetch`, and it is the same dropped
  // connection either way.
  let calls = 0;
  let delivered = 0;
  const dropsMidBody = async () => {
    calls += 1;
    if (calls > 1) return new Response(MODEL);
    return new Response(
      new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array(MODEL).subarray(0, 4));
        },
        // The error belongs here rather than beside the enqueue above:
        // `controller.error` clears whatever is still queued, so erroring in
        // `start` delivers no chunk at all and tests the same thing the
        // rejected-fetch case already does. Erroring on the pull that follows
        // the first read is what makes this a failure partway through.
        pull(controller) {
          // Reached only once the queued chunk has been read, which is what
          // makes the failure below a mid-body one and is why it is counted.
          delivered += 1;
          controller.error(new TypeError("network error"));
        },
      }),
    );
  };

  const bytes = await withFetch(dropsMidBody, () => modelBytes("model-url", null, PIN));
  assert.equal(await sha256Hex(bytes), PIN);
  assert.equal(calls, 2, "a half-delivered body is worth one more try");
  assert.equal(delivered, 1, "and the first attempt really did get partway in");
});

test("the pinned path is a commit and not a branch", () => {
  // A branch moves, and the pin then turns every later interview's avatar into
  // the neutral panel until somebody re-reads the hash. `docs/avatar-contract.md`
  // claims the path survives a force-push, and that claim is only true of a
  // commit-pinned one.
  assert.match(
    MODEL_URL,
    /\/[0-9a-f]{40}\//,
    "MODEL_URL must name a full commit sha, not a branch or a tag",
  );
});

for (const [why, respond] of [
  ["before the response arrives", () => {
    throw new TypeError("Failed to fetch");
  }],
  ["partway through the body", () =>
    new Response(
      new ReadableStream({
        start(controller) {
          controller.error(new TypeError("network error"));
        },
      }),
    )],
]) {
  test(`an expired deadline is not retried: ${why}`, async () => {
    // The failure looks exactly like a dropped connection, which is retried,
    // and the browsers do not agree on the name that would tell them apart. So
    // the signal is asked instead, and this is what proves it is asked: an
    // already-aborted deadline turns a retryable failure into a final one. A
    // second attempt would carry the same expired signal and fail before it
    // reached the network, having told the candidate nothing new.
    const expired = new AbortController();
    expired.abort();
    let calls = 0;

    await withFetch(
      async () => {
        calls += 1;
        return respond();
      },
      async () => {
        await assert.rejects(
          () => modelBytes("model-url", null, PIN, expired.signal),
          /model-url/,
        );
      },
    );
    assert.equal(calls, 1, "a deadline that has passed does not pass again");
  });
}

test("a body larger than the first allocation and shorter than the ceiling arrives whole", async () => {
  // No `Content-Length`, so the read starts on a buffer it has to outgrow, and
  // the bytes are distinct rather than zeroes so that a growth step which loses
  // or repeats a run changes the hash instead of hiding in it.
  const size = 200 * 1024;
  const full = new Uint8Array(size);
  // Random rather than a per-byte loop, which cost more than everything this
  // test is about. `getRandomValues` fills 64 KiB at a time, and the pin is
  // taken from the same array, so the bytes only have to be distinct.
  for (let at = 0; at < size; at += 65536) crypto.getRandomValues(full.subarray(at, at + 65536));
  const pin = await sha256Hex(full.buffer);

  const bytes = await withFetch(sends(full, 8 * 1024), () => modelBytes("model-url", null, pin));
  assert.equal(bytes.byteLength, size, "no trailing slack from the buffer it grew into");
  assert.equal(await sha256Hex(bytes), pin);
});

test("the fetch carries the deadline, and both attempts carry the same one", async () => {
  // Without this the whole timeout is decoration: every other test here
  // fabricates the expiry itself, so removing `signal` from the fetch leaves
  // them all green while a hung origin holds the tab for the life of the page.
  // The second half is the other way it goes wrong: a signal made per attempt
  // gives a slow origin twice the budget the panel waits.
  const signals = [];
  let calls = 0;
  const noticesTheSignal = async (url, init) => {
    signals.push(init?.signal);
    calls += 1;
    if (calls === 1) throw new TypeError("network error");
    return new Response(MODEL);
  };

  const bytes = await withFetch(noticesTheSignal, () => modelBytes("model-url", null, PIN));
  assert.equal(await sha256Hex(bytes), PIN);
  assert.equal(signals.length, 2);
  assert.ok(signals[0] instanceof AbortSignal, "the fetch must be given a deadline at all");
  assert.equal(signals[0], signals[1], "one deadline for the load, not one per attempt");
  assert.equal(signals[0].aborted, false, "and it is a live deadline, not a spent one");
});

test("a response with no body at all is refused rather than hashed", async () => {
  // A 204 is `ok`, declares nothing, and carries no stream to read. Reaching
  // `getReader()` on it throws where the pin should have spoken.
  await withFetch(async () => ({ ok: true, status: 204, headers: new Headers(), body: null }), async () => {
    await assert.rejects(() => modelBytes("model-url", null, PIN), /answered 204 with no body/);
  });
});

test("a reader that will not let go costs a connection, not the avatar", async () => {
  // `cancel` is called on the way out of a refusal, and a reader that throws
  // from it synchronously would otherwise replace the refusal with its own
  // error, which is the failure `store` below has the same shape to avoid.
  const wontRelease = async () => ({
    ok: true,
    status: 200,
    headers: new Headers(),
    body: {
      getReader() {
        return {
          async read() {
            return { done: true, value: undefined };
          },
          cancel() {
            throw new TypeError("released before cancel");
          },
        };
      },
    },
  });

  await withFetch(wontRelease, async () => {
    // An empty body, so what surfaces is the pin and not the cancel.
    await assert.rejects(
      () => modelBytes("model-url", null, PIN),
      /did not match its pinned SHA-256/,
    );
  });
});

test("a sweep that cannot even list costs disk, not the load", async () => {
  // `evictSuperseded` is fired and not awaited, so a rejection escaping it is
  // an unhandled rejection in the candidate's tab rather than a failed sweep.
  await evictSuperseded({
    async keys() {
      throw new DOMException("storage went away", "InvalidStateError");
    },
    async delete() {
      return true;
    },
  });
});

test("a blip followed by a refusal reports the refusal, not the blip", async () => {
  // The one retry path nothing else drives: the two attempts failing for
  // different reasons. What the candidate's console gets has to be the second
  // answer, because the first one is the transient this retry exists to absorb.
  let calls = 0;
  await withFetch(
    async () => {
      calls += 1;
      if (calls === 1) throw new TypeError("network error");
      return new Response("gone", { status: 404 });
    },
    async () => {
      await assert.rejects(() => modelBytes("model-url", null, PIN), /answered 404/);
    },
  );
  assert.equal(calls, 2);
});

test("an unreadable Content-Length is quoted back trimmed, not entire", async () => {
  // The value is the origin's, unbounded, and interpolated into a string. Today
  // that string reaches a console; the trim is so that a change which puts it
  // somewhere else -- a DOM node, a report field -- does not inherit an
  // attacker-sized payload along with it.
  const enormous = "n".repeat(5000);
  const declaresAnEssay = async () => ({
    ok: true,
    status: 200,
    headers: new Headers({ "content-length": enormous }),
    body: null,
  });

  await withFetch(declaresAnEssay, async () => {
    const error = await modelBytes("model-url", null, PIN).catch((caught) => caught);
    assert.match(error.message, /unreadable Content-Length/);
    assert.ok(
      error.message.length < 200,
      `the refusal quoted ${error.message.length} characters of a header the origin chose`,
    );
  });
});

test("the default deadline is the renderer's own, and it is a timeout", async () => {
  // The seam the tests use to expire a deadline without waiting a minute would
  // also let the real one be a signal that never fires, and every other test
  // here would stay green. So the constructor is watched once: what a caller
  // who names no deadline gets has to be `AbortSignal.timeout` of the same
  // number the neutral panel waits, which is the pairing the whole feature
  // rests on.
  const asked = [];
  const real = AbortSignal.timeout;
  AbortSignal.timeout = (ms) => {
    asked.push(ms);
    return real.call(AbortSignal, ms);
  };
  try {
    const bytes = await withFetch(async () => new Response(MODEL), () =>
      modelBytes("model-url", null, PIN));
    assert.equal(await sha256Hex(bytes), PIN);
  } finally {
    AbortSignal.timeout = real;
  }

  assert.deepEqual(asked, [LOAD_TIMEOUT_MS], "one timeout, of the panel's own length");
});

test("a warm cache hit builds no deadline it will not use", async () => {
  // A default parameter would be evaluated before the cache read, leaving the
  // browser holding a 60-second timer for a load that returned in microseconds.
  const asked = [];
  const real = AbortSignal.timeout;
  AbortSignal.timeout = (ms) => {
    asked.push(ms);
    return real.call(AbortSignal, ms);
  };
  try {
    const bytes = await modelBytes("model-url", fakeCache(MODEL), PIN);
    assert.equal(await sha256Hex(bytes), PIN);
  } finally {
    AbortSignal.timeout = real;
  }

  assert.deepEqual(asked, [], "the network was never reached, so nothing needed a deadline");
});
