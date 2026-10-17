// A tab-local draw identity survives navigation without relying on the clock.
const key = "codetrial.random-draw";
const snapshotKey = `${key}.lobby`;
let outcomeDraw = null;
const outcomeKey = (draw) => `${key}.outcome.${draw.id}`;

// Same-origin receipts distinguish the current save from retained older rows.
// Ticket-specific keys keep concurrent tabs from replacing each other's result.
function readSaveOutcome(draw) {
  try {
    const area = globalThis.localStorage;
    const value = area.getItem(outcomeKey(draw));
    if (!value) return null;
    // Read-only storage cannot prove that a later failure invalidated this row.
    area.setItem(outcomeKey(draw), value);
    const outcome = JSON.parse(value);
    if (outcome.id !== draw.id || outcome.problemId !== draw.problemId)
      return null;
    outcomeDraw = draw;
    return outcome;
  } catch {
    return null;
  }
}

function clearSaveOutcome(draw) {
  if (!draw) return;
  try {
    globalThis.localStorage.removeItem(outcomeKey(draw));
  } catch {
    /* no unavailable receipt is inferred as completion */
  }
}

function writeSaveOutcome(draw, completed, reportId = null) {
  outcomeDraw = draw;
  try {
    globalThis.localStorage.setItem(
      outcomeKey(draw),
      JSON.stringify({
        id: draw.id,
        problemId: draw.problemId,
        completed,
        reportId,
      }),
    );
  } catch {
    clearSaveOutcome(draw);
  }
}

export function beginRandomDrawSave(draw, storage) {
  if (!draw) return;
  writeSaveOutcome(draw, false);
  try {
    storage ??= globalThis.sessionStorage;
    if (readRandomDraw(draw.problemId, storage)?.id !== draw.id) return;
    // Removing the old success first prevents quota failures retaining it.
    storage.removeItem(key);
    storage.setItem(
      key,
      JSON.stringify({ id: draw.id, problemId: draw.problemId }),
    );
  } catch {
    /* no storage completion is inferred */
  }
}

export function createRandomDraw(problemId) {
  const id = globalThis.crypto.getRandomValues(new Uint32Array(4)).join("-");
  return { id, problemId };
}

export function storeRandomDraw(draw, storage) {
  if (outcomeDraw?.id !== draw?.id) clearSaveOutcome(outcomeDraw);
  if (draw) writeSaveOutcome(draw, false);
  else outcomeDraw = null;
  try {
    storage ??= globalThis.sessionStorage;
    storage.removeItem(snapshotKey);
    if (draw === null) storage.removeItem(key);
    else
      storage.setItem(
        key,
        JSON.stringify({ id: draw.id, problemId: draw.problemId }),
      );
    return true;
  } catch {
    return false;
  }
}

export function readRandomDraw(problemId, storage) {
  try {
    storage ??= globalThis.sessionStorage;
    const draw = JSON.parse(storage.getItem(key));
    return typeof draw?.id === "string" &&
      draw.id.length > 0 &&
      draw.problemId === problemId
      ? draw
      : null;
  } catch {
    return null;
  }
}

export function completeRandomDraw(draw, entry, storage) {
  if (
    !draw ||
    entry?.problemId !== draw.problemId ||
    typeof entry.id !== "string" ||
    !entry.id ||
    entry.report?.incomplete ||
    !["HIRE", "NO_HIRE"].includes(entry.report?.decision)
  )
    return false;
  if (readSaveOutcome(draw)) writeSaveOutcome(draw, true, entry.id);
  try {
    storage ??= globalThis.sessionStorage;
    const active = readRandomDraw(draw.problemId, storage);
    if (active?.id !== draw.id) return false;
    storage.setItem(
      key,
      JSON.stringify({
        id: draw.id,
        problemId: draw.problemId,
        completed: true,
        reportId: entry.id,
        interviewId: entry.interviewId ?? null,
      }),
    );
    return true;
  } catch {
    return false;
  }
}

export function consumeRandomDraw(draw, storage) {
  if (!draw) return false;
  const outcome = readSaveOutcome(draw);
  if (outcome && outcome.completed !== true) return false;
  const active = readRandomDraw(draw.problemId, storage);
  if (
    active?.id !== draw.id ||
    active.completed !== true ||
    typeof active.reportId !== "string" ||
    !active.reportId
  )
    return false;
  try {
    storage ??= globalThis.sessionStorage;
    // A read-only marker may predate a save that could not invalidate it.
    storage.setItem(key, JSON.stringify(active));
  } catch {
    return false;
  }
  storeRandomDraw(null, storage);
  return true;
}

// Match persisted history to the latest save receipt, not an older success.
export function randomDrawCompleted(draw, reports) {
  const outcome = draw && readSaveOutcome(draw);
  return (
    draw !== null &&
    outcome?.completed === true &&
    reports.some(
      (entry) =>
        entry.id === outcome.reportId &&
        entry.randomDrawId === draw.id &&
        entry.problemId === draw.problemId &&
        typeof entry.id === "string" &&
        entry.id.length > 0 &&
        !entry.report?.incomplete &&
        ["HIRE", "NO_HIRE"].includes(entry.report?.decision),
    )
  );
}

// Leave and Done open a new history entry, so its state cannot carry the lobby.
export function storeRandomDrawSnapshot(snapshot, storage) {
  try {
    storage ??= globalThis.sessionStorage;
    if (snapshot === null) storage.removeItem(snapshotKey);
    else storage.setItem(snapshotKey, JSON.stringify(snapshot));
    return true;
  } catch {
    return false;
  }
}

export function readRandomDrawSnapshot(storage) {
  try {
    storage ??= globalThis.sessionStorage;
    const snapshot = JSON.parse(storage.getItem(snapshotKey));
    return snapshot?.draw &&
      readRandomDraw(snapshot.draw.problemId, storage)?.id === snapshot.draw.id
      ? snapshot
      : null;
  } catch {
    return null;
  }
}
