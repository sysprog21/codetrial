import { test } from "node:test";
import assert from "node:assert/strict";
import {
  completeRandomDraw,
  consumeRandomDraw,
  createRandomDraw,
  readRandomDraw,
  randomDrawCompleted,
  storeRandomDraw,
} from "../../web/random-draw.js";
import { reportIsPersisted } from "../../web/history.js";
import { functionBody, memoryStorage, read } from "./source.js";

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
    randomDrawTicket: readRandomDraw("problem", area),
    tabStorage: area,
    completeRandomDraw,
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
    randomDrawTicket: draw,
    tabStorage: {
      getItem() {
        throw new Error("blocked");
      },
    },
    completeRandomDraw,
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

test("the interview captures its URL ticket without accessing blocked tab storage", () => {
  const draw = createRandomDraw("problem");
  const declaration = read("web/interview.js").match(
    /^const randomDrawTicket =[\s\S]*?;/m,
  )[0];
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
  const area = memoryStorage();
  storeRandomDraw(draw, area);
  assert.deepEqual(capture(params, area, readRandomDraw), draw);
});
