export const historyKey = "codetrial_history";
export const reviewHistoryKey = "codetrial_review_history";

// Full reports are what lets an older attempt reopen after the short history
// rolls over. Keep as many as fit in this budget, up to 500, rather than
// pretending every report has the same small serialized size.
const REVIEW_HISTORY_CAP = 500;
const REVIEW_HISTORY_BYTES = 164 * 1024;

/// Every request here is a small same-origin call, and every one of them is
/// awaited by something the candidate is looking at: a stalled save leaves the
/// report page saying "Saving report..." with no end, and a stalled clear leaves
/// the Delete button disabled, because the finally that re-enables it never runs.
/// A deadline turns both into the failure they already know how to report.
const requestTimeoutMs = 10_000;

/// Always a list. The key sits in the origin's local storage, which an older
/// build, another tab, or anyone with devtools open can write, so what it holds
/// is input rather than something this module chose. Unparseable JSON was
/// already answered with an empty list, but parseable JSON that is not one went
/// straight through to callers that all treat it as an array.
export function readLocalHistory(storage) {
  return storedList(historyKey, storage) ?? [];
}

/// The list a store holds, or null when it holds anything else.
///
/// Null rather than an empty list because two callers have to tell "no list
/// stored yet" from "a list that is empty": one rebuilds the review store in
/// that case and one declines to rewrite a store that is not there. The rest
/// fold both into an empty list at the call.
///
/// No fallback for a missing key: `JSON.parse(null)` answers null, which is
/// not a list, and the shape check already turns that into one.
function storedList(key, storage) {
  try {
    const stored = JSON.parse((storage || localStorage).getItem(key));
    return Array.isArray(stored) ? stored : null;
  } catch {
    return null;
  }
}

export function readReviewHistory(storage) {
  try {
    storage ||= localStorage;
    const stored = storedList(reviewHistoryKey, storage);
    if (stored) return stored;
    const { retained, json } = boundedReviews(readLocalHistory(storage));
    try {
      storage.setItem(reviewHistoryKey, json);
    } catch {
      /* the readable history still belongs on screen */
    }
    return retained;
  } catch {
    return [];
  }
}

/// Everything this device has, newest first.
///
/// Two stores, one answer, because neither is a superset of the other. The
/// short history keeps the 20 newest attempts whatever they weigh, while
/// `boundedReviews` skips a report too large for the byte budget, so an attempt
/// the lobby can still reopen was missing from the review store and the panel
/// drawn from it alone. The review store extends the 20 with the older attempts
/// the history has rolled past, and those are all older than anything in it, so
/// the two concatenate newest first without a sort.
///
/// The history is taken whole and only the review store is filtered against it.
/// Two attempts that serialize alike are still two attempts, and a rule that
/// deduplicated within a store quietly merged them.
export function readDeviceHistory(storage) {
  try {
    storage ||= localStorage;
    const history = readLocalHistory(storage);
    const reviews = readReviewHistory(storage);
    // Mutates both arrays, and writes both stores when it has anything to say.
    migrateLegacyIds(history, reviews, storage);
    const shown = new Set(
      history.map((entry) => entry?.id).filter((id) => id != null),
    );
    // `shown` never holds a nullish id, so a row without one is kept by the miss.
    return [...history, ...reviews.filter((entry) => !shown.has(entry?.id))];
  } catch {
    return [];
  }
}

/// Give pre-id attempts one durable identity in both stores before merging.
/// `problemId` and `pageMapChecked` are excluded only while pairing the copies:
/// a refused second rename write may have changed those fields in one store.
function migrateLegacyIds(history, reviews, storage) {
  // Every report this app has saved carries an id, so the usual answer is that
  // there is nothing to pair. Decide that from the rows themselves, before
  // building a content key for each one and walking one store against the
  // other: serializing a full review store to answer "no" costs more than the
  // read it is part of.
  const unidentified = (entry) => pairable(entry) && entry.id == null;
  const historyChanged = history.some(unidentified);
  const reviewsChanged = reviews.some(unidentified);
  if (!historyChanged && !reviewsChanged) return;

  // One history row pairs with one review row, so a matched candidate is
  // withdrawn rather than left to match a second time.
  const candidates = history
    .filter(pairable)
    .map((entry) => ({ entry, key: legacyKey(entry), used: false }));
  for (const review of reviews) {
    if (!pairable(review)) continue;
    const key = legacyKey(review);
    const match = candidates.find(
      (candidate) => !candidate.used && candidate.key === key,
    );
    const id = match?.entry.id ?? review.id ?? assignedId();
    if (match) {
      match.used = true;
      match.entry.id ??= id;
    }
    review.id ??= id;
  }
  for (const { entry } of candidates) entry.id ??= assignedId();

  if (historyChanged) {
    try {
      storage.setItem(historyKey, JSON.stringify(history));
    } catch {
      /* the ids still apply to this read */
    }
  }
  if (reviewsChanged) {
    try {
      const serialized = JSON.stringify(reviews);
      if (
        reviews.length <= REVIEW_HISTORY_CAP &&
        new TextEncoder().encode(serialized).length <= REVIEW_HISTORY_BYTES
      ) {
        storage.setItem(reviewHistoryKey, serialized);
      }
    } catch {
      /* the ids still apply to this read */
    }
  }
}

/// An id for a row that predates them, in the shape `randomId` in interview.js
/// gives every report saved since.
///
/// Not `crypto.randomUUID`: it exists only in a secure context, so on an
/// http:// origin it is undefined and calling it throws. That throw reached
/// `readDeviceHistory`'s catch, which answered with no history at all, and the
/// next save then wrote a store built from nothing. Losing a candidate's
/// attempts because the page was served over plain HTTP is a far worse trade
/// than the collision odds this gives up, which over tens of rows are nil.
export function assignedId() {
  return Math.random().toString(36).slice(2);
}

/// Whether a stored row is a record this can pair at all. Both stores are
/// parsed from text anyone with devtools can edit, so a row may be anything.
function pairable(entry) {
  return entry != null && typeof entry === "object" && !Array.isArray(entry);
}

/// What two copies of one attempt have in common. `problemId` and
/// `pageMapChecked` are excluded because `renameLocalHistory` writes its two
/// stores through separate calls, so a refused second write leaves exactly
/// those two fields disagreeing between the copies.
function legacyKey(entry) {
  // Copied field by field rather than spread-then-deleted: a `delete` drops the
  // copy into dictionary mode and the stringify that follows it pays for that,
  // which is most of what this function used to cost. The target has a null
  // prototype so a stored `__proto__` stays an ordinary field of the key
  // instead of silently becoming the object's prototype and vanishing from it.
  const stable = Object.create(null);
  for (const field of Object.keys(entry)) {
    if (field === "id" || field === "problemId" || field === "pageMapChecked")
      continue;
    stable[field] = entry[field];
  }
  return JSON.stringify(stable);
}

/// History saved before problems had page names carries published ids, and a
/// lobby translating them fetches the page map on every visit. Rewritten once
/// here, the next visit finds page names and fetches nothing. `pages` is that
/// map; entries it does not name are left as they are.
///
/// Both local stores, by one rule. The review list is read against page names
/// too, and an interview saved before the first lobby visit builds it from the
/// unrenamed history, so renaming only the history left those reviews matching
/// no card. Only an existing review list is rewritten: a missing one is built
/// from the history when it is first read, after this has run.
export function renameLocalHistory(pages, storage, markUnmapped = false) {
  try {
    storage ||= localStorage;
    const history = renamedEntries(
      readLocalHistory(storage),
      pages,
      markUnmapped,
    );
    if (history.renamed)
      storage.setItem(historyKey, JSON.stringify(history.next));
    const stored = storedList(reviewHistoryKey, storage);
    if (stored) {
      const reviews = renamedEntries(stored, pages, markUnmapped);
      // Bounded on the way back out. A page name is not the same length as the
      // published id it replaces, so a rename can push a store that was inside
      // the budget past it, and the write that finally fails is a later save
      // rather than this one.
      if (reviews.renamed) {
        storage.setItem(reviewHistoryKey, boundedReviews(reviews.next).json);
      }
    }
    return history.next;
  } catch {
    return readLocalHistory(storage);
  }
}

function renamedEntries(entries, pages, markUnmapped) {
  let renamed = false;
  const next = entries.map((entry) => {
    const id = entry?.problemId;
    if (typeof id !== "string" || !Object.hasOwn(pages, id)) {
      if (
        !markUnmapped ||
        typeof id !== "string" ||
        entry?.pageMapChecked === true
      )
        return entry;
      renamed = true;
      return { ...entry, pageMapChecked: true };
    }
    renamed = true;
    return { ...entry, problemId: pages[id].page };
  });
  return { next, renamed };
}

export async function saveReportHistory(
  entry,
  { fetcher = fetch, storage } = {},
) {
  const local = saveLocalReport(entry, storage);
  const account = await inOrder(entry?.id, () =>
    saveAccountReport(entry, fetcher),
  );
  return { local, account };
}

/// At least one copy the Past attempts flow can read was written successfully.
export function reportIsPersisted(result) {
  return result?.local === "saved" || result?.account === "saved";
}

// Account saves of one report id, in the order they were asked for. A report
// saved again under its id -- an outcome over the provisional failure it
// replaces -- otherwise raced the first save's session check, and whichever
// POST landed last was the copy the account kept.
const accountSaves = new Map();

function inOrder(id, save) {
  if (id == null) return save();
  const next = (accountSaves.get(id) ?? Promise.resolve()).then(save, save);
  accountSaves.set(id, next);
  const forget = () => {
    if (accountSaves.get(id) === next) accountSaves.delete(id);
  };
  next.then(forget, forget);
  return next;
}

/// `account` is what the page knows: whether the history it is showing came
/// from an account. The session is still rechecked here rather than trusted
/// from page load, because signing in from another tab has to reach the
/// account copy. The pair matters in the other direction too: signing out from
/// another tab leaves a copy this browser can no longer authenticate a delete
/// for, and clearing the local half of that would report an erasure that only
/// happened on this device.
export async function clearReportHistory({
  account = false,
  fetcher = fetch,
  storage,
} = {}) {
  try {
    const session = await sessionState(fetcher);
    if (session === "failed") return "failed";
    if (session === "in") {
      const deleted = await fetcher("/api/reports", {
        method: "DELETE",
        signal: AbortSignal.timeout(requestTimeoutMs),
      });
      if (!deleted.ok) return "failed";
    } else if (account !== false) {
      return "failed";
    }
    try {
      storage ||= localStorage;
      storage.removeItem(historyKey);
      storage.removeItem(reviewHistoryKey);
      return "cleared";
    } catch {
      return session === "in" ? "account-cleared-local-failed" : "failed";
    }
  } catch {
    return "failed";
  }
}

/// One report, named by the id its copies share rather than by where it sits in
/// a list. The list on screen is filtered and both stores hold it in their own
/// order, so a position means a different report in each of the three.
///
/// Every copy goes, from both stores: `readDeviceHistory` merges them on this
/// id, so a copy left in either one draws the attempt again on the next read.
/// The session is rechecked for the reasons `clearReportHistory` gives, and the
/// answers are its answers, plus "missing" when neither the account nor either
/// store held the id. A row `migrateLegacyIds` gave an id it could not write
/// back is one: both stores still hold it without that id, so it is drawn
/// again on the next read, and "deleted" reported a removal that never ran.
export async function deleteReport(
  id,
  { account = false, fetcher = fetch, storage } = {},
) {
  if (typeof id !== "string" || id === "") return "failed";
  try {
    const session = await sessionState(fetcher);
    if (session === "failed") return "failed";
    let fromAccount = false;
    if (session === "in") {
      const deleted = await fetcher(`/api/reports/${encodeURIComponent(id)}`, {
        method: "DELETE",
        signal: AbortSignal.timeout(requestTimeoutMs),
      });
      if (!deleted.ok) return "failed";
      // Only a count of 0 is a miss. A 200 whose body did not parse still ran
      // the delete, and calling that missing would hide a removal.
      const body = await deleted.json().catch(() => null);
      fromAccount = body?.deleted !== 0;
    } else if (account !== false) {
      return "failed";
    }
    let removed = fromAccount;
    try {
      storage ||= localStorage;
      // A missing review store is left missing: it is rebuilt from the short
      // history on its first read, and that no longer holds the report.
      for (const key of [historyKey, reviewHistoryKey]) {
        const stored = storedList(key, storage);
        if (!stored) continue;
        // Compared as `normalizeProgressEntry` spells the id, which is where
        // the one the candidate clicked came from.
        const kept = stored.filter(
          (entry) => entry?.id == null || String(entry.id) !== id,
        );
        if (kept.length === stored.length) continue;
        storage.setItem(key, JSON.stringify(kept));
        removed = true;
      }
      return removed ? "deleted" : "missing";
    } catch {
      return fromAccount ? "account-deleted-local-failed" : "failed";
    }
  } catch {
    return "failed";
  }
}

function saveLocalReport(entry, storage) {
  try {
    storage ||= localStorage;
    const previous = readLocalHistory(storage);
    // The short history first, and before the review store is even read.
    // Reading it can rebuild it, and that write is the 164 KB one: spending the
    // remaining quota on it here refused the save of a report the device had
    // room for, and the candidate was told it was gone.
    // A save under an id already held replaces that row: an interview's final
    // outcome is saved over the provisional failure it kept while the report
    // could still be regenerated.
    const others = previous.filter(
      (row) => entry?.id == null || row?.id !== entry.id,
    );
    storage.setItem(
      historyKey,
      JSON.stringify([entry, ...others].slice(0, 20)),
    );
    // The short history is what the lobby draws this report from, so a review
    // store that refuses the write costs the reopen twenty attempts from now,
    // not this save. Reporting "Report was not saved" for it told the candidate
    // their report was gone while it sat in the panel behind them.
    //
    // Rebuilt from the merged view rather than from the review store alone, so
    // an attempt whose own review write was refused goes back in the next time
    // one lands, instead of being lost the moment it rolls past the twenty.
    try {
      storage.setItem(
        reviewHistoryKey,
        boundedReviews(readDeviceHistory(storage)).json,
      );
    } catch {
      /* the report is on this device either way */
    }
    return "saved";
  } catch {
    return "failed";
  }
}

// One report at a time against a running total, rather than re-serializing
// everything retained so far on each step: that measured the same 164 KB five
// hundred times over on a single save. A review too large for what is left is
// skipped and the walk carries on, so one outsized report costs itself and not
// the older attempts behind it.
//
// Without the final code. It stays in the short history, so the newest twenty
// reopen with it, while the budget here buys as many older attempts as before.
function boundedReviews(reviews) {
  const encoder = new TextEncoder();
  const retained = [];
  const texts = [];
  // The brackets the array itself serializes to, before any element.
  let bytes = 2;
  for (const row of reviews.slice(0, REVIEW_HISTORY_CAP)) {
    const review = withoutCode(row);
    const text = JSON.stringify(review);
    // A separator for every element after the first.
    const size = encoder.encode(text).length + (retained.length ? 1 : 0);
    if (bytes + size > REVIEW_HISTORY_BYTES) continue;
    bytes += size;
    retained.push(review);
    texts.push(text);
  }
  // The text as well as the rows. Measuring already serialized every retained
  // report, so a caller that writes `JSON.stringify(retained)` serializes all
  // five hundred of them a second time for the same string.
  return { retained, json: `[${texts.join(",")}]` };
}

/// A stored row with its final code left out, and any other row as it is.
/// Copied field by field for the reason `legacyKey` gives, and defined rather
/// than assigned: assigning a stored `__proto__` field would set the copy's
/// prototype instead and drop the field from the row written back. Not a
/// null-prototype object as there, because these rows go on to the lobby.
function withoutCode(entry) {
  if (!pairable(entry) || !Object.hasOwn(entry, "code")) return entry;
  const copy = {};
  for (const field of Object.keys(entry)) {
    if (field === "code") continue;
    Object.defineProperty(copy, field, {
      value: entry[field],
      enumerable: true,
      writable: true,
      configurable: true,
    });
  }
  return copy;
}

/// The account copy carries no code: README.md says code snapshots stay in
/// memory unless recording is enabled, and the server keeps this body as sent.
async function saveAccountReport(entry, fetcher) {
  const session = await sessionState(fetcher);
  if (session === "out") return "skipped";
  if (session === "failed") return "failed";
  try {
    const saved = await fetcher("/api/reports", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(withoutCode(entry)),
      signal: AbortSignal.timeout(requestTimeoutMs),
    });
    return saved.ok ? "saved" : "failed";
  } catch {
    return "failed";
  }
}

async function sessionState(fetcher) {
  try {
    const response = await fetcher("/api/session", {
      signal: AbortSignal.timeout(requestTimeoutMs),
    });
    if (!response.ok) return "failed";
    const session = await response.json();
    if (session.signedIn === true) return "in";
    if (session.signedIn === false) return "out";
    return "failed";
  } catch {
    return "failed";
  }
}
