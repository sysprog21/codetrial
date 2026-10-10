// A tab-local draw identity survives navigation without relying on the clock.
const key = "codetrial.random-draw";

export function createRandomDraw(problemId) {
  const id = globalThis.crypto.getRandomValues(new Uint32Array(4)).join("-");
  return { id, problemId };
}

export function storeRandomDraw(draw, storage) {
  try {
    storage ??= globalThis.sessionStorage;
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
  const active = readRandomDraw(draw.problemId, storage);
  if (
    active?.id !== draw.id ||
    active.completed !== true ||
    typeof active.reportId !== "string" ||
    !active.reportId
  )
    return false;
  storeRandomDraw(null, storage);
  return true;
}

// The report carries the navigation ticket when tab storage cannot carry it.
export function randomDrawCompleted(draw, reports) {
  return (
    draw !== null &&
    reports.some(
      (entry) =>
        entry.randomDrawId === draw.id &&
        entry.problemId === draw.problemId &&
        typeof entry.id === "string" &&
        entry.id.length > 0 &&
        !entry.report?.incomplete &&
        ["HIRE", "NO_HIRE"].includes(entry.report?.decision),
    )
  );
}
