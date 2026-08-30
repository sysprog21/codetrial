export const historyKey = "codetrial_history";

/// Always a list. The key sits in the origin's local storage, which an older
/// build, another tab, or anyone with devtools open can write, so what it holds
/// is input rather than something this module chose. Unparseable JSON was
/// already answered with an empty list, but parseable JSON that is not one went
/// straight through to callers that all treat it as an array.
export function readLocalHistory(storage = localStorage) {
  try {
    // No fallback for a missing key: `JSON.parse(null)` answers null, which is
    // not a list, and the shape check below already turns that into one.
    const stored = JSON.parse(storage.getItem(historyKey));
    return Array.isArray(stored) ? stored : [];
  } catch {
    return [];
  }
}

export async function saveReportHistory(entry, { fetcher = fetch, storage = localStorage } = {}) {
  saveLocalReport(entry, storage);
  if (!(await signedIn(fetcher))) return false;
  return syncReport(entry, fetcher);
}

function saveLocalReport(entry, storage) {
  try {
    const previous = readLocalHistory(storage);
    storage.setItem(historyKey, JSON.stringify([entry, ...previous].slice(0, 20)));
  } catch {
  }
}

async function signedIn(fetcher) {
  try {
    const response = await fetcher("/api/session");
    if (!response.ok) return false;
    const session = await response.json();
    return session.signedIn === true;
  } catch {
    return false;
  }
}

async function syncReport(entry, fetcher) {
  try {
    const response = await fetcher("/api/reports", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(entry),
    });
    return response.ok;
  } catch {
    return false;
  }
}
