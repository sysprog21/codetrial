export const historyKey = "codetrial_history";

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

export async function saveReportHistory(entry, { fetcher = fetch, storage } = {}) {
  const local = saveLocalReport(entry, storage);
  const account = await saveAccountReport(entry, fetcher);
  return { local, account };
}

function saveLocalReport(entry, storage) {
  try {
    storage ||= localStorage;
    const previous = readLocalHistory(storage);
    storage.setItem(historyKey, JSON.stringify([entry, ...previous].slice(0, 20)));
    return "saved";
  } catch {
    return "failed";
  }
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
    });
    return saved.ok ? "saved" : "failed";
  } catch {
    return "failed";
  }
}

async function sessionState(fetcher) {
  try {
    const response = await fetcher("/api/session");
    if (!response.ok) return "failed";
    const session = await response.json();
    if (session.signedIn === true) return "in";
    if (session.signedIn === false) return "out";
    return "failed";
  } catch {
    return "failed";
  }
}
