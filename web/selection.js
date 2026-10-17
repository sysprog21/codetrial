// One previous assignment per tab. This is selection metadata, never recovery
// state, and the server checks its signature and owner before using it.
export const STRICT_ASSIGNMENT_KEY = "codetrial.strictAssignment";

const storageError =
  "Strict Interview needs session storage to remember its previous assignment. Allow site storage and reload, or return to the lobby.";

export function previousAssignment(storage) {
  try {
    if (!storage) throw new Error();
    const previous = storage.getItem(STRICT_ASSIGNMENT_KEY);
    // Test writes before asking for consent or allocating a room. Restore the
    // original absence rather than leaving a probe looking like a receipt.
    storage.setItem(STRICT_ASSIGNMENT_KEY, previous ?? "");
    if (storage.getItem(STRICT_ASSIGNMENT_KEY) !== (previous ?? ""))
      throw new Error();
    if (previous === null) storage.removeItem(STRICT_ASSIGNMENT_KEY);
    return previous;
  } catch {
    throw new Error(storageError);
  }
}

export function rememberAssignment(storage, receipt) {
  if (typeof receipt !== "string" || !receipt || receipt.length > 512)
    throw new Error("The server returned an invalid Strict assignment.");
  try {
    storage.setItem(STRICT_ASSIGNMENT_KEY, receipt);
    if (storage.getItem(STRICT_ASSIGNMENT_KEY) !== receipt) throw new Error();
  } catch {
    throw new Error(storageError);
  }
}

export function resetAssignments(storage) {
  previousAssignment(storage);
  storage.removeItem(STRICT_ASSIGNMENT_KEY);
}
