// The model's bytes: where they come from, and the check that they are the
// right ones.
//
// Split out of vrm.js because none of this touches Three.js, and vrm.js is the
// one file in the repo that does. That is also what makes it testable: the
// cache branches, the pin, and the storage failures below are the parts most
// likely to break silently, and here `node --test` can drive all of them
// against a fake cache without a browser or a renderer. The one reference to
// the renderer is a dynamic import at the bottom, which no test reaches.

// Which bytes are Jim, and where they come from. Both live here, together,
// because they are one fact: a model swap changes the URL and the hash in the
// same edit, and splitting them across two modules only created a pair that
// could drift. `src/web/policy.rs` holds a third copy of the origin, which the
// language boundary forces; a test pins it against this one.
export const MODEL_URL =
  "https://raw.githubusercontent.com/vrm-c/vrm-specification/837f156dbce43ad69183ce1bdab549961ae1c1ee/samples/Seed-san/vrm/Seed-san.vrm";
export const MODEL_SHA256 = "624d0d554bc205bbdc33e22a68a2c3c20edebb3e573011ead8878a65e5329b23";

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
export async function modelBytes(url, cache, expected) {
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

  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url} answered ${response.status}`);
  const bytes = await response.arrayBuffer();
  if ((await sha256Hex(bytes)) !== expected) {
    throw new Error(`${url} did not match its pinned SHA-256`);
  }
  // Not awaited: the write is bookkeeping for a later interview, and `new
  // Response` copies 11 MB before the storage round-trip even begins. Nothing
  // downstream reads it back, which the swallowed failure is the proof of.
  void store(cache, url, bytes);
  return bytes;
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
