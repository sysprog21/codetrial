import { test } from "node:test";
import assert from "node:assert/strict";
import {
  beginRandomDrawSave,
  completeRandomDraw,
  consumeRandomDraw,
  createRandomDraw,
  readRandomDraw,
  readRandomDrawSnapshot,
  storeRandomDrawSnapshot,
  randomDrawCompleted,
  storeRandomDraw,
} from "../../web/random-draw.js";
import { reportIsPersisted } from "../../web/history.js";
import { functionBody, memoryStorage, read } from "./source.js";

globalThis.localStorage = memoryStorage();

const entry = (decision = "HIRE") => ({
  id: "report",
  problemId: "problem",
  interviewId: "interview",
  report: { decision },
});

for (const decision of ["HIRE", "NO_HIRE"]) {
  test(`a completed ${decision} interview ends its own draw exactly once`, () => {
    const area = memoryStorage();
    const draw = createRandomDraw("problem");
    storeRandomDraw(draw, area);
    assert.equal(completeRandomDraw(draw, entry(decision), area), true);
    assert.equal(consumeRandomDraw(draw, area), true);
    assert.equal(consumeRandomDraw(draw, area), false);
  });
}

test("a stale interview cannot complete a newer draw of the same problem", () => {
  const area = memoryStorage();
  const old = createRandomDraw("problem");
  storeRandomDraw(old, area);
  const current = createRandomDraw("problem");
  storeRandomDraw(current, area);
  assert.notEqual(old.id, current.id);
  assert.equal(completeRandomDraw(old, entry(), area), false);
  assert.equal(consumeRandomDraw(current, area), false);
  assert.equal(completeRandomDraw(current, entry(), area), true);
  assert.equal(consumeRandomDraw(old, area), false);
  assert.equal(consumeRandomDraw(current, area), true);
});

for (const invalid of [
  { ...entry(), report: { decision: "NO_HIRE", incomplete: true } },
  { ...entry(), report: { decision: "PENDING" } },
  { ...entry(), problemId: "another-problem" },
  { ...entry(), id: "" },
]) {
  test(`an incomplete or unrelated report does not complete a draw: ${JSON.stringify(invalid)}`, () => {
    const area = memoryStorage();
    const draw = createRandomDraw("problem");
    storeRandomDraw(draw, area);
    assert.equal(completeRandomDraw(draw, invalid, area), false);
    assert.equal(consumeRandomDraw(draw, area), false);
  });
}

test("an unreadable marker and blocked storage fail without throwing", () => {
  const area = {
    getItem() {
      throw new Error("blocked");
    },
    setItem() {
      throw new Error("blocked");
    },
    removeItem() {
      throw new Error("blocked");
    },
  };
  const draw = createRandomDraw("problem");
  assert.equal(storeRandomDraw(draw, area), false);
  assert.equal(readRandomDraw("problem", area), null);
  assert.equal(completeRandomDraw(draw, entry(), area), false);
  assert.equal(consumeRandomDraw(draw, area), false);
  assert.equal(readRandomDraw("problem", { getItem: () => "{" }), null);
});

test("blocked access to the sessionStorage getter does not throw", () => {
  const previous = Object.getOwnPropertyDescriptor(
    globalThis,
    "sessionStorage",
  );
  Object.defineProperty(globalThis, "sessionStorage", {
    configurable: true,
    get() {
      throw new Error("blocked");
    },
  });
  try {
    const draw = createRandomDraw("problem");
    assert.equal(storeRandomDraw(draw), false);
    assert.equal(readRandomDraw("problem"), null);
    assert.equal(completeRandomDraw(draw, entry()), false);
    assert.equal(consumeRandomDraw(draw), false);
  } finally {
    if (previous) Object.defineProperty(globalThis, "sessionStorage", previous);
    else delete globalThis.sessionStorage;
  }
});

test("saving an actual interview report records its captured draw ticket", async () => {
  const area = memoryStorage();
  const draw = createRandomDraw("problem");
  storeRandomDraw(draw, area);
  let saved;
  const guardStates = [];
  const scope = {
    state: {
      reportId: null,
      report: { decision: "HIRE" },
      interviewId: "interview",
    },
    randomId: () => "report",
    problem: { page: "problem", title: "Problem", difficulty: "Easy" },
    whiteboard: true,
    durationMin: 10,
    interviewLoop: "reacto",
    mode: "whiteboard",
    randomDrawTicket: readRandomDraw("problem", area),
    tabStorage: area,
    completeRandomDraw,
    beginRandomDrawSave,
    saveGeneration: 0,
    reportIsPersisted,
    updateBeforeUnloadGuard: () =>
      guardStates.push(scope.state.reportPersisted),
    saveReportHistory: (value) => {
      saved = value;
      return Promise.resolve({ local: "saved", account: "skipped" });
    },
  };
  const save = new Function(
    "scope",
    `with (scope) { ${functionBody(read("web/interview.js").replace(/\r\n/g, "\n"), "saveHistory")}\n}\nreturn saveHistory; }`,
  )(scope);
  assert.deepEqual(await save(), { local: "saved", account: "skipped" });
  assert.deepEqual(guardStates, [false, true]);
  assert.equal(scope.state.reportPersisted, true);
  assert.equal(saved.id, "report");
  assert.equal(saved.interviewId, "interview");
  assert.equal(consumeRandomDraw(draw, area), true);
});

for (const decision of ["HIRE", "NO_HIRE"]) {
  test(`a saved ${decision} report completes its draw without tab storage`, () => {
    const draw = createRandomDraw("problem");
    const report = { ...entry(decision), randomDrawId: draw.id };
    storeRandomDraw(draw, {
      removeItem() {
        throw new Error("blocked");
      },
    });
    completeRandomDraw(draw, report, {
      getItem() {
        throw new Error("blocked");
      },
    });
    assert.equal(randomDrawCompleted(draw, [report]), true);
    for (const unrelated of [
      { ...report, randomDrawId: createRandomDraw("problem").id },
      { ...report, problemId: "another-problem" },
      { ...report, id: "" },
      { ...report, report: { decision, incomplete: true } },
      { ...report, report: { decision: "PENDING" } },
      entry(decision),
    ]) {
      assert.equal(randomDrawCompleted(draw, [unrelated]), false);
    }
    assert.equal(randomDrawCompleted(draw, []), false);
    assert.equal(randomDrawCompleted(null, [report]), false);
  });
}

test("saving an interview with blocked tab storage preserves completion in its report", async () => {
  const draw = createRandomDraw("problem");
  storeRandomDraw(draw, {
    removeItem() {
      throw new Error("blocked");
    },
  });
  let saved;
  const guardStates = [];
  const scope = {
    state: {
      reportId: null,
      report: { decision: "NO_HIRE" },
      interviewId: null,
    },
    randomId: () => "report",
    problem: { page: "problem", title: "Problem", difficulty: "Easy" },
    whiteboard: true,
    durationMin: 10,
    interviewLoop: "reacto",
    mode: "whiteboard",
    randomDrawTicket: draw,
    tabStorage: {
      getItem() {
        throw new Error("blocked");
      },
    },
    completeRandomDraw,
    beginRandomDrawSave,
    saveGeneration: 0,
    reportIsPersisted,
    updateBeforeUnloadGuard: () =>
      guardStates.push(scope.state.reportPersisted),
    saveReportHistory: (value) => {
      saved = value;
      return Promise.resolve({ local: "saved", account: "skipped" });
    },
  };
  const save = new Function(
    "scope",
    `with (scope) { ${functionBody(read("web/interview.js").replace(/\r\n/g, "\n"), "saveHistory")}\n}\nreturn saveHistory; }`,
  )(scope);
  assert.deepEqual(await save(), { local: "saved", account: "skipped" });
  assert.deepEqual(guardStates, [false, true]);
  assert.equal(scope.state.reportPersisted, true);
  assert.equal(saved.randomDrawId, draw.id);
  assert.equal(randomDrawCompleted(draw, [saved]), true);
});

test("the interview takes its draw ticket only from its URL", () => {
  const draw = createRandomDraw("problem");
  const declaration = read("web/interview.js").match(
    /^const randomDrawTicket =[\s\S]*?;/m,
  )[0];
  const area = memoryStorage();
  storeRandomDraw(draw, area);
  const capture = new Function(
    "params",
    "tabStorage",
    "readRandomDraw",
    declaration + "\nreturn randomDrawTicket;",
  );
  const params = new URLSearchParams({
    problem: draw.problemId,
    draw: draw.id,
  });
  assert.deepEqual(
    capture(params, null, () => {
      throw new Error("blocked");
    }),
    draw,
  );
  params.delete("draw");
  assert.equal(capture(params, area, readRandomDraw), null);
  assert.equal(consumeRandomDraw(draw, area), false);
});

function reportSaveHarness(saveReportHistory, area = memoryStorage()) {
  const draw = createRandomDraw("problem");
  storeRandomDraw(draw, area);
  const scope = {
    state: {
      reportId: null,
      report: { decision: "HIRE" },
      interviewId: "interview",
    },
    randomId: () => "report",
    problem: { page: "problem", title: "Problem", difficulty: "Easy" },
    whiteboard: true,
    durationMin: 10,
    interviewLoop: "reacto",
    mode: "whiteboard",
    randomDrawTicket: draw,
    tabStorage: area,
    completeRandomDraw,
    beginRandomDrawSave,
    saveGeneration: 0,
    reportIsPersisted,
    updateBeforeUnloadGuard: () => {},
    saveReportHistory,
  };
  const save = new Function(
    "scope",
    `with (scope) { ${functionBody(read("web/interview.js").replace(/\r\n/g, "\n"), "saveHistory")}\n}\nreturn saveHistory; }`,
  )(scope);
  return { save, area, draw, state: scope.state };
}

for (const failure of ["unsaved", "rejected"]) {
  test(`a ${failure} report save does not complete its random draw`, async () => {
    const harness = reportSaveHarness(async () => {
      if (failure === "rejected") throw new Error("network failure");
      return { local: "failed", account: "failed" };
    });
    if (failure === "rejected")
      await assert.rejects(harness.save(), /network failure/);
    else await harness.save();
    assert.equal(harness.state.reportPersisted, false);
    assert.equal(consumeRandomDraw(harness.draw, harness.area), false);
  });
}

test("a pending report save completes its draw only after persistence", async () => {
  let resolve;
  const harness = reportSaveHarness(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const pending = harness.save();
  assert.equal(
    readRandomDraw(harness.draw.problemId, harness.area).completed,
    undefined,
  );
  resolve({ local: "saved", account: "skipped" });
  await pending;
  assert.equal(harness.state.reportPersisted, true);
  assert.equal(consumeRandomDraw(harness.draw, harness.area), true);
});

test("an older successful save cannot complete a draw after a newer save fails", async () => {
  const pending = [];
  const harness = reportSaveHarness(
    () => new Promise((done) => pending.push(done)),
  );
  const older = harness.save();
  const newer = harness.save();
  pending[0]({ local: "saved", account: "skipped" });
  await older;
  assert.equal(
    readRandomDraw(harness.draw.problemId, harness.area).completed,
    undefined,
  );
  pending[1]({ local: "failed", account: "failed" });
  await newer;
  assert.equal(harness.state.reportPersisted, false);
  assert.equal(consumeRandomDraw(harness.draw, harness.area), false);
});

test("a lobby snapshot belongs only to its active draw and survives grading", () => {
  const area = memoryStorage();
  const draw = createRandomDraw("problem");
  const snapshot = { draw, difficulties: ["Hard"], duration: 60 };
  storeRandomDraw(draw, area);
  assert.equal(storeRandomDrawSnapshot(snapshot, area), true);
  assert.deepEqual(readRandomDrawSnapshot(area), snapshot);
  completeRandomDraw(draw, entry(), area);
  assert.deepEqual(readRandomDrawSnapshot(area), snapshot);
  assert.equal(consumeRandomDraw(draw, area), true);
  assert.equal(readRandomDrawSnapshot(area), null);
  assert.equal(area.getItem("codetrial.random-draw.lobby"), null);
  storeRandomDraw(draw, area);
  storeRandomDrawSnapshot(snapshot, area);
  storeRandomDraw(createRandomDraw("problem"), area);
  assert.equal(readRandomDrawSnapshot(area), null);
  storeRandomDrawSnapshot(snapshot, area);
  assert.equal(readRandomDrawSnapshot(area), null);
  storeRandomDraw(null, area);
  assert.equal(area.getItem("codetrial.random-draw.lobby"), null);
});

test("malformed snapshots and blocked storage cannot interrupt lobby restoration", () => {
  const area = memoryStorage();
  area.setItem("codetrial.random-draw.lobby", "{");
  assert.equal(readRandomDrawSnapshot(area), null);
  const blocked = {
    getItem() {
      throw new Error("blocked");
    },
    setItem() {
      throw new Error("blocked");
    },
    removeItem() {
      throw new Error("blocked");
    },
  };
  assert.equal(readRandomDrawSnapshot(blocked), null);
  assert.equal(
    storeRandomDrawSnapshot({ draw: createRandomDraw("problem") }, blocked),
    false,
  );
  assert.equal(storeRandomDrawSnapshot(null, blocked), false);
});

for (const blocked of [false, true]) {
  test(`a newer failed save revokes an older completed draw with storage ${blocked ? "blocked" : "available"}`, async () => {
    const area = blocked
      ? {
          getItem() {
            throw new Error("blocked");
          },
          setItem() {
            throw new Error("blocked");
          },
          removeItem() {
            throw new Error("blocked");
          },
        }
      : memoryStorage();
    let saved;
    let saves = 0;
    const harness = reportSaveHarness(async (value) => {
      if (++saves === 1) {
        saved = value;
        return { local: "saved", account: "skipped" };
      }
      return { local: "failed", account: "failed" };
    }, area);
    await harness.save();
    assert.equal(randomDrawCompleted(harness.draw, [saved]), true);
    await harness.save();
    assert.equal(randomDrawCompleted(harness.draw, [saved]), false);
    assert.equal(consumeRandomDraw(harness.draw, area), false);
  });
}

test("a read-only outcome cannot reuse an older successful receipt", () => {
  const area = memoryStorage();
  const draw = createRandomDraw("problem");
  storeRandomDraw(draw, area);
  const report = { ...entry(), randomDrawId: draw.id };
  completeRandomDraw(draw, report, area);
  const writable = globalThis.localStorage;
  globalThis.localStorage = {
    getItem: (key) => writable.getItem(key),
    setItem() {
      throw new Error("read only");
    },
    removeItem() {
      throw new Error("read only");
    },
  };
  try {
    beginRandomDrawSave(draw, {
      getItem() {
        throw new Error("blocked");
      },
    });
    assert.equal(randomDrawCompleted(draw, [report]), false);
  } finally {
    globalThis.localStorage = writable;
  }
});

test("save receipts for another tab cannot replace this draw's result", async () => {
  const other = await import("../../web/random-draw.js?other-tab");
  const draw = createRandomDraw("problem");
  const area = memoryStorage();
  storeRandomDraw(draw, area);
  const report = { ...entry(), randomDrawId: draw.id };
  completeRandomDraw(draw, report, area);
  const second = other.createRandomDraw("problem");
  other.storeRandomDraw(second, memoryStorage());
  assert.equal(randomDrawCompleted(draw, [report]), true);
  assert.equal(other.randomDrawCompleted(second, [report]), false);
});

test("read-only completion stores cannot retain a success after a failed newer save", () => {
  const area = memoryStorage();
  const draw = createRandomDraw("problem");
  storeRandomDraw(draw, area);
  const report = { ...entry(), randomDrawId: draw.id };
  completeRandomDraw(draw, report, area);
  const local = globalThis.localStorage;
  const readonly = (storage) => ({
    getItem: (key) => storage.getItem(key),
    setItem() {
      throw new Error("read only");
    },
    removeItem() {
      throw new Error("read only");
    },
  });
  globalThis.localStorage = readonly(local);
  try {
    const tab = readonly(area);
    beginRandomDrawSave(draw, tab);
    assert.equal(randomDrawCompleted(draw, [report]), false);
    assert.equal(consumeRandomDraw(draw, tab), false);
  } finally {
    globalThis.localStorage = local;
  }
});
