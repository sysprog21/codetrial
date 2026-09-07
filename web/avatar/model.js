// The model's bytes: where they come from, and the check that they are the
// right ones.
//
// Split out of vrm.js because none of this touches Three.js, and vrm.js is the
// one file in the repo that does. That is also what makes it testable: the
// cache branches, the pin, and the storage failures below are the parts most
// likely to break silently, and here `node --test` can drive all of them
// against a fake cache without a browser or a renderer. Two references to the
// renderer half: a dynamic import of `vrm.js` at the bottom, which no test
// reaches and which is where Three.js enters, and the load deadline imported
// from `avatar.js` below, which costs nothing because that file is arithmetic.

// The deadline, from the renderer rather than beside it. `LOAD_TIMEOUT_MS` is
// when the neutral panel appears, and that race cannot cancel anything, so a
// second copy of the number here would be the fetch promising to stop at a time
// somebody could move without it. Free to import: `avatar.js` imports nothing
// and touches no `document` at module scope, so this pulls no renderer into a
// `node --test` run.
import { LOAD_TIMEOUT_MS } from "./avatar.js";

// Which bytes are Jim, and where they come from. Both live here, together,
// because they are one fact: a model swap changes the URL and the hash in the
// same edit, and splitting them across two modules only created a pair that
// could drift. `src/web/policy.rs` holds a third copy of the origin, which the
// language boundary forces; a test pins it against this one.
export const MODEL_URL =
  "https://raw.githubusercontent.com/vrm-c/vrm-specification/837f156dbce43ad69183ce1bdab549961ae1c1ee/samples/Seed-san/vrm/Seed-san.vrm";
export const MODEL_SHA256 = "624d0d554bc205bbdc33e22a68a2c3c20edebb3e573011ead8878a65e5329b23";

// What the fetch will accept, in binary megabytes. `docs/avatar-contract.md`
// sets 5 to 15 MB as the budget for a commissioned replacement and records the
// model above at 10,917,800 bytes, so this is that budget's top and leaves
// today's model 4.8 MB of headroom. It widens the budget's scope, which the
// document now says beside the number: it was what a commission may weigh, and
// it is also what a candidate's tab will accept from the origin. Exported and
// named so that raising it is a decision somebody makes in a diff.
export const MODEL_MAX_BYTES = 15 * 1024 * 1024;

// The hash is in the name, so a model swap can never read a stale entry, and
// the prefix is what makes the superseded cache findable for deletion. The
// pattern is built from the prefix rather than repeating it, or the constant
// would be claiming to be a single definition while sitting beside a copy.
const CACHE_PREFIX = "codetrial-avatar-";
const SUPERSEDED_CACHE = new RegExp(`^${CACHE_PREFIX}[0-9a-f]{64}$`);
export const MODEL_CACHE = `${CACHE_PREFIX}${MODEL_SHA256}`;

async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// Storage, not integrity, which is why every failure here returns null instead
/// of throwing, and why `modelBytes` swallows the read side too. `caches.open`
/// rejects outright in Firefox private browsing and `cache.put` rejects with
/// QuotaExceededError once 11 MB will not fit, and both of those describe a
/// browser that can still download and render the model perfectly well. Losing
/// the cache costs a download; refusing to load over it would cost the avatar.
export async function openModelCache(caches = globalThis.caches) {
  // Outside the try: it cannot reject, and a catch that covers more than the
  // calls that can fail hides the ones that do.
  if (!caches) return null;
  try {
    const cache = await caches.open(MODEL_CACHE);
    void evictSuperseded(caches);
    return cache;
  } catch {
    return null;
  }
}

/// Garbage collection, so it is not awaited. Nothing depends on its result, and
/// on the one load that finds something the candidate would otherwise wait for
/// an 11 MB delete before the download of the model they actually need starts.
///
/// Exported for the test, which has to await it: asserting on the side effect
/// of a call nobody awaits would pass or fail on scheduling luck.
export async function evictSuperseded(caches) {
  try {
    for (const name of await caches.keys()) {
      if (SUPERSEDED_CACHE.test(name) && name !== MODEL_CACHE) await caches.delete(name);
    }
  } catch {
    // A sweep that cannot run costs disk, not correctness.
  }
}

/// Every byte that reaches the caller is hashed first, including bytes that
/// came back from the cache. A cached entry is only as trustworthy as whatever
/// last wrote to this origin's storage, and re-hashing 11 MB costs tens of
/// milliseconds against a download that costs seconds, so there is no reason to
/// trust one and check the other.
///
/// `expected` is a parameter and not a closed-over constant for exactly one
/// reason: the pin names one 11 MB file that no test can produce, so without
/// this the success path, the storage failure, and the poisoned-cache recovery
/// are all unreachable from `node --test` and only the throwing branches get
/// covered. It has no default, so the seam cannot be reached accidentally: the
/// only caller that means the real model has to name it.
///
/// `deadline` is optional and `expected` is not, and the difference is what each
/// costs if a caller gets it wrong. A wrong pin would be an integrity failure,
/// so naming it is mandatory. A wrong deadline is a wait, and the only caller
/// that wants a different one is a test: expiring a real 60-second signal is not
/// something `node --test` can sit through.
export async function modelBytes(url, cache, expected, deadline) {
  // The whole read is inside the catch, not just the open. `match` and the body
  // read reject on a storage backend that went away mid-session, and `delete`
  // rejects for the same reasons `put` does. Every one of those is a cache that
  // stopped working, which is a reason to download and not a reason to fail, so
  // this falls through to the network exactly as a cold load would.
  try {
    const hit = await cache?.match(url);
    if (hit) {
      const bytes = await hit.arrayBuffer();
      if ((await sha256Hex(bytes)) === expected) return bytes;
      // Truncated by an interrupted write, or replaced. Either way it is not
      // the model, so drop it and pay for the download rather than failing.
      await cache.delete(url);
    }
  } catch {
    // Unreadable, so unread.
  }

  // Made here rather than in the signature, because a default parameter is
  // evaluated on every call: a warm cache hit would build a timer nobody waits
  // on, and the browser would hold it for a minute after the load it was for
  // had already returned.
  //
  // One deadline for the load, not one per attempt: a retry that renewed it
  // would let a slow origin hold the candidate for twice as long as the panel
  // waits before giving up on it. Without any deadline, which is what this had,
  // an origin trickling a byte a minute never reaches the ceiling and never
  // finishes either, and the socket, the buffer and a retry all outlive the
  // panel for the rest of the page's life.
  //
  // The retry never goes back through `cache.match`. A poisoned entry read
  // twice fails the second time for the reason it failed the first, and the
  // read above has already deleted it anyway.
  deadline ??= AbortSignal.timeout(LOAD_TIMEOUT_MS);
  const bytes = await onceMore(() => download(url, expected, deadline), deadline);
  // Not awaited: the write is bookkeeping for a later interview, and `new
  // Response` copies 11 MB before the storage round-trip even begins. Nothing
  // downstream reads it back, which the swallowed failure is the proof of.
  void store(cache, url, bytes);
  return bytes;
}

/// A failure a second attempt could answer differently, and the only kind
/// `onceMore` repeats. There are exactly two: the transport dropped the
/// request, and the bytes that arrived were not the model. Everything else this
/// module throws is an answer the origin will give again.
///
/// A class rather than a flag on the error, because a flag is a property
/// anything that touched the error on the way here could have set, and the
/// question being asked is whether this module raised it.
class Transient extends Error {}

/// One retry and only one. A mismatch has already discarded what it had and a
/// dropped connection leaves the state it started from, so neither has anything
/// to undo first. One transient blip must not cost a candidate the avatar for
/// the interview they are sitting, and two attempts at an origin that has
/// already given its answer must not cost them the wait.
///
/// The deadline outranks the class. A dropped connection and an expired
/// deadline arrive at the same two `catch` blocks as the same kind of failure,
/// and the browsers do not agree on how to tell them apart -- `AbortSignal`
/// expiry is documented as a `TimeoutError`, which is what Chromium and Node
/// report, and Firefox reports it as an `AbortError`. Asking the signal is both
/// portable and the truth: the second attempt would carry the same signal and
/// fail before it reached the network, so there is nothing to buy.
///
/// The second attempt is outside the `try`, so there is no third.
async function onceMore(attempt, deadline) {
  try {
    return await attempt();
  } catch (error) {
    if (!(error instanceof Transient) || deadline.aborted) throw error;
    return attempt();
  }
}

/// One trip to the network: the URL that was asked for, a ceiling checked
/// before the body and again while it streams, and the pin over what arrived.
async function download(url, expected, deadline) {
  let response;
  try {
    // `redirect: "error"` pins the response to the URL that was asked for. CSP
    // is enforced against each redirect target, but its allowlist is wider than
    // this origin: `content_security_policy` in `src/web/policy.rs` also names
    // Compiler Explorer and every LiveKit URL, so a redirect to one of those
    // passes `connect-src` and still reaches the pin. The pin would refuse the
    // bytes, but only after a candidate's tab has bought all of them.
    response = await fetch(url, { redirect: "error", signal: deadline });
  } catch (cause) {
    // A dropped connection and a refused redirect are the same event from here.
    // Both reject with a bare `TypeError` carrying no machine-readable reason,
    // in every browser and in Node, so the redirect gets the retry a blip gets
    // and is refused again on the second attempt. An expired deadline lands
    // here too, and `onceMore` is where that is told apart, by asking the
    // signal rather than by reading a name the browsers spell differently.
    throw new Transient(`${url} could not be fetched`, { cause });
  }
  // Released for the same reason the declared length below is: an error page
  // has a body too, and it is already arriving. Throwing straight past it
  // leaves the connection open behind a load that has already failed.
  if (!response.ok) {
    void release(response.body);
    throw new Error(`${url} answered ${response.status}`);
  }

  const bytes = await capped(url, response);
  if ((await sha256Hex(bytes)) !== expected) {
    throw new Transient(`${url} did not match its pinned SHA-256`);
  }
  return bytes;
}

/// The length the origin claims, refused here rather than downstream, because
/// this is the one check that can happen before a byte is read: an origin that
/// announces what it is about to send is turned away without sending it.
///
/// Read strictly. HTTP allows one unsigned decimal, and `Number` is laxer in
/// three ways that all come out under the ceiling: two joined values, which is
/// what a duplicate header becomes, are `NaN`; four hundred digits are
/// `Infinity`, which is not finite; and an empty header is zero. A declaration
/// that cannot be read is a broken origin declaring nothing, which is a reason
/// to stop rather than to guess. The streamed cap below would catch the body
/// either way, but only after the transfer this exists to avoid.
///
/// Returns the length when there is a readable one, so the read below can
/// allocate once. `null` means the origin did not say, which is every chunked
/// response.
function declaredBytes(url, response) {
  const declared = response.headers.get("content-length");
  if (declared === null) return null;
  if (!/^\d+$/.test(declared.trim())) {
    // Truncated because the value is the origin's, unbounded, and about to be
    // interpolated into a string somebody may one day put somewhere else.
    throw new Error(`${url} declared an unreadable Content-Length: ${declared.slice(0, 64)}`);
  }
  const length = Number(declared);
  if (length > MODEL_MAX_BYTES) {
    throw new Error(`${url} declared ${length} bytes, over the ${MODEL_MAX_BYTES} byte ceiling`);
  }
  return length;
}

/// Reads the body a chunk at a time and stops at the ceiling. `arrayBuffer()`
/// cannot do this: by the time it returns, the tab has already bought every
/// byte the origin chose to send, and a declared length is a claim rather than
/// a limit -- it is the compressed size when the response is compressed, and it
/// is absent altogether on a chunked one.
///
/// One buffer that grows, rather than an array of chunks joined at the end.
/// How many chunks a body arrives in is the origin's choice and not the size's,
/// and an array pays a view, a buffer and a slot for each one: a body sent a
/// byte at a time costs gigabytes of heap while `total` still reads a few
/// megabytes and the ceiling never fires. Copying each chunk in and letting it
/// go makes the cost proportional to the bytes, which is what the ceiling is
/// counting. A declared length that got past `declaredBytes` is the exact size
/// in the ordinary case, so the usual load allocates once, grows never, and
/// hands its own buffer back without a second copy.
async function capped(url, response) {
  // First, before the body is touched at all: that is the whole value of a
  // declared length, and reading it after the check below would change which
  // refusal a bodyless response carrying a bad one gives.
  //
  // Released on the way out, because the body is already arriving by the time
  // the header is read. Throwing straight past it leaves the transfer running
  // behind a load that has failed, holding the connection and buying the bytes
  // this ceiling exists to refuse.
  let declared;
  try {
    declared = declaredBytes(url, response);
  } catch (refusal) {
    void release(response.body);
    throw refusal;
  }
  if (!response.body) throw new Error(`${url} answered ${response.status} with no body`);
  const reader = response.body.getReader();
  let bytes = new Uint8Array(declared ?? 64 * 1024);
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await readChunk(url, reader);
      if (done) break;
      const needed = total + value.byteLength;
      if (needed > MODEL_MAX_BYTES) {
        throw new Error(`${url} sent more than the ${MODEL_MAX_BYTES} byte ceiling`);
      }
      if (needed > bytes.byteLength) {
        // The clamp can never land below `needed`, because the line above has
        // just established that `needed` fits under the ceiling.
        //
        // How often this runs is a cost and not a behavior, and no test here
        // observes it: growing exactly to `needed` every time, or starting from
        // 64 KiB while a truthful length was available, produce the same bytes
        // and the same refusals, only slower. The two decisions that are
        // behavior, the ceiling and the trim, are each pinned by a test.
        const wanted = Math.max(bytes.byteLength * 2, needed);
        const grown = new Uint8Array(Math.min(wanted, MODEL_MAX_BYTES));
        grown.set(bytes.subarray(0, total));
        bytes = grown;
      }
      // `set` copies the chunk's own visible range, so a view onto a larger
      // buffer contributes only its own bytes, which is what a real stream
      // hands out.
      bytes.set(value, total);
      total += value.byteLength;
    }
  } finally {
    // Not awaited, for the reason the ceiling exists: a stream whose `cancel`
    // never settles would hold the refusal open exactly as long as the transfer
    // it was refusing. Nothing here needs the release to complete.
    void release(reader);
  }

  // `crypto.subtle` and the cache both want exactly what arrived and nothing
  // after it, so a buffer the body did not fill is trimmed. An origin that
  // declared its length honestly skips this copy, and the pinned one does:
  // `raw.githubusercontent.com` sends an exact `Content-Length` and no
  // `Content-Encoding` for this path, so the ordinary load allocates once and
  // returns its own buffer. Handing back `bytes.subarray(0, total)` would skip
  // the copy on the other path too -- `digest` and `new Response` both take a
  // view -- but `GLTFLoader.parseAsync` in `vrm.js` branches on
  // `instanceof ArrayBuffer` and would quietly read a view as JSON, so the copy
  // goes when that caller does and not before.
  return total === bytes.byteLength ? bytes.buffer : bytes.buffer.slice(0, total);
}

/// A connection that drops eight megabytes in is the same event as one that
/// drops before the first byte, and the retry has to see it as one: a body read
/// rejects with the same bare `TypeError` the fetch itself would have, and with
/// the same exception an expired deadline gives. The read is the only call in
/// the loop that fails that way, so it is the only one wrapped, and the
/// ceiling's own refusal beside it stays a plain `Error`.
async function readChunk(url, reader) {
  try {
    return await reader.read();
  } catch (cause) {
    throw new Transient(`${url} stopped sending partway through the body`, { cause });
  }
}

/// Its own function for the same reason `store` below is: one `catch` that
/// covers a `cancel` which throws as well as one which rejects. It takes either
/// the stream or a reader on it, because the two refusals reach it from
/// different places: a declared length is judged before any reader exists, and
/// the streamed ceiling fires once one does. Either way a body that will not
/// let go costs a connection, never the load.
async function release(body) {
  try {
    await body?.cancel();
  } catch {
    // Unreleased, and nothing downstream of this cares.
  }
}

/// Its own function, and `async`, so that one `catch` covers every way storing
/// can fail. A bare `.catch()` on the returned promise would miss a `put` that
/// throws synchronously, and losing the cache must never cost the avatar.
async function store(cache, url, bytes) {
  try {
    await cache?.put(url, new Response(bytes));
  } catch {
    // Unstorable, not unusable. The next interview downloads it again.
  }
}

/// Refuses rather than degrades when `crypto.subtle` is missing, which means an
/// insecure context: `http://` on a LAN address is exactly how this gets
/// demonstrated, and it is the one place where quietly loading 11 MB of
/// unverified third-party geometry would matter most. A browser that cannot
/// check the pin gets the neutral panel, which is a state the page already has.
export async function loadModelBytes() {
  if (!globalThis.crypto?.subtle) {
    throw new Error("the avatar model cannot be verified without a secure context");
  }
  return modelBytes(MODEL_URL, await openModelCache(), MODEL_SHA256);
}

/// Bytes first, then the renderer, in the one place that knows both.
///
/// Both orders are serial, so fetching the model first costs the same when it
/// is reachable and saves the 730 KB Three.js bundle when it is not. Having it
/// here rather than at each call site is what keeps that true: the interview
/// page and the recording page both want it, and an invariant copied into two
/// entry points is one that gets reversed in the entry point nobody tested.
///
/// `vrm.js` is still reached by dynamic import and only from here, so nothing
/// pulls Three.js into a `node --test` run that merely imports this module.
export async function loadAvatarModel(mount) {
  const bytes = await loadModelBytes();
  const renderer = await import("./vrm.js");
  return renderer.loadVrm({ mount, bytes });
}
