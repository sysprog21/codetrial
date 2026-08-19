export const historyKey = "codetrial_history";

export function readLocalHistory(storage = localStorage) {
  try {
    return JSON.parse(storage.getItem(historyKey) ?? "[]");
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
