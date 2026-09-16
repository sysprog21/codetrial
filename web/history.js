export const historyKey = "codetrial_history";
export const reviewHistoryKey = "codetrial_review_history";

// JSON.stringify of the widest retained record below is 328 bytes; 500 rows
// therefore reserve at most about 164 KB, while the full history remains 20.
const REVIEW_HISTORY_CAP = 500;

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
  try {
    storage ||= localStorage;
    // No fallback for a missing key: `JSON.parse(null)` answers null, which is
    // not a list, and the shape check below already turns that into one.
    const stored = JSON.parse(storage.getItem(historyKey));
    return Array.isArray(stored) ? stored : [];
  } catch {
    return [];
  }
}

export function readReviewHistory(storage) {
  try {
    storage ||= localStorage;
    const stored = JSON.parse(storage.getItem(reviewHistoryKey));
    if (Array.isArray(stored)) return stored;
    const rebuilt = readLocalHistory(storage).map(reviewEntry);
    storage.setItem(reviewHistoryKey, JSON.stringify(rebuilt));
    return rebuilt;
  } catch {
    return [];
  }
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
    const history = renamedEntries(readLocalHistory(storage), pages, markUnmapped);
    if (history.renamed) storage.setItem(historyKey, JSON.stringify(history.next));
    const stored = JSON.parse(storage.getItem(reviewHistoryKey));
    if (Array.isArray(stored)) {
      const reviews = renamedEntries(stored, pages, markUnmapped);
      if (reviews.renamed) storage.setItem(reviewHistoryKey, JSON.stringify(reviews.next));
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
      if (!markUnmapped || typeof id !== "string" || entry?.pageMapChecked === true) return entry;
      renamed = true;
      return { ...entry, pageMapChecked: true };
    }
    renamed = true;
    return { ...entry, problemId: pages[id].page };
  });
  return { next, renamed };
}

export async function saveReportHistory(entry, { fetcher = fetch, storage } = {}) {
  const local = saveLocalReport(entry, storage);
  const account = await saveAccountReport(entry, fetcher);
  return { local, account };
}

/// `account` is what the page knows: whether the history it is showing came
/// from an account. The session is still rechecked here rather than trusted
/// from page load, because signing in from another tab has to reach the
/// account copy. The pair matters in the other direction too: signing out from
/// another tab leaves a copy this browser can no longer authenticate a delete
/// for, and clearing the local half of that would report an erasure that only
/// happened on this device.
export async function clearReportHistory({ account = false, fetcher = fetch, storage } = {}) {
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

function saveLocalReport(entry, storage) {
  try {
    storage ||= localStorage;
    const previous = readLocalHistory(storage);
    const reviews = readReviewHistory(storage);
    storage.setItem(historyKey, JSON.stringify([entry, ...previous].slice(0, 20)));
    storage.setItem(reviewHistoryKey, JSON.stringify([reviewEntry(entry), ...reviews].slice(0, REVIEW_HISTORY_CAP)));
    return "saved";
  } catch {
    return "failed";
  }
}

function reviewEntry(entry) {
  const report = entry?.report;
  return {
    problemId: entry?.problemId,
    date: entry?.date,
    report: {
      decision: report?.decision,
      incomplete: report?.incomplete,
      improvementPlan: report?.improvementPlan,
    },
  };
}

async function saveAccountReport(entry, fetcher) {
  const session = await sessionState(fetcher);
  if (session === "out") return "skipped";
  if (session === "failed") return "failed";
  try {
    const saved = await fetcher("/api/reports", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(entry),
      signal: AbortSignal.timeout(requestTimeoutMs),
    });
    return saved.ok ? "saved" : "failed";
  } catch {
    return "failed";
  }
}

async function sessionState(fetcher) {
  try {
    const response = await fetcher("/api/session", { signal: AbortSignal.timeout(requestTimeoutMs) });
    if (!response.ok) return "failed";
    const session = await response.json();
    if (session.signedIn === true) return "in";
    if (session.signedIn === false) return "out";
    return "failed";
  } catch {
    return "failed";
  }
}
