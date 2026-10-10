import test from "node:test";
import assert from "node:assert/strict";
import { ALL_LANGUAGES } from "../../web/compiler-explorer.js";
import {
  LANGUAGE_LABELS,
  acceptTaskState,
  outcomeSentence,
  rulesInWords,
  taskLocked,
  taskCodeFileName,
  taskRecovery,
  taskReviewForDisplay,
} from "../../web/task-controller.js";
import {
  taskActionPayload,
  taskEndPayload,
  taskRevisionPayload,
  taskRunPayload,
  taskStartPayload,
} from "../../web/lib.js";
import { read } from "./source.js";

const state = (seq = 1) => ({
  version: 1,
  seq,
  phase: "work",
  checks: [],
  finalRevisionId: null,
});
const contract = { package: 1, prompt: 1, wire: 1, reportSchema: 1, rubric: 1 };

test("unknown review contracts and interview reports display feedback unavailable", () => {
  const valid = {
    assessmentMode: "task",
    taskContract: contract,
    feedbackUnavailable: true,
    taskAssessment: { sessionId: "task-room" },
  };
  assert.equal(taskReviewForDisplay(valid), valid);
  for (const value of [
    { codingScore: 80 },
    { ...valid, taskContract: { ...contract, reportSchema: 2 } },
    { ...valid, feedbackUnavailable: false, taskAssessment: {} },
  ]) {
    assert.equal(taskReviewForDisplay(value).feedbackUnavailable, true);
    assert.equal(taskReviewForDisplay(value).taskAssessment, undefined);
  }
});

test("an invalid or interrupted result is accepted for display as sent", () => {
  for (const outcome of ["invalid", "interrupted"]) {
    const value = {
      assessmentMode: "task",
      taskContract: contract,
      taskAssessment: {
        outcome: { outcome, cause: "fullscreen", at: 75, durationMs: 0 },
      },
    };
    assert.equal(taskReviewForDisplay(value), value);
  }
});

test("only new authoritative task states update progress", () => {
  const first = state();
  assert.equal(acceptTaskState(first, state()), first);
  assert.equal(
    acceptTaskState(first, { ...state(2), phase: "invalid" }),
    first,
  );
  assert.equal(acceptTaskState(first, state(2)).seq, 2);
});

test("editing is locked outside work and after the final revision", () => {
  assert.equal(taskLocked(state()), false);
  assert.equal(taskLocked(null), true);
  assert.equal(taskLocked({ ...state(), phase: "ready" }), true);
  assert.equal(taskLocked({ ...state(), finalRevisionId: "final" }), true);
  assert.equal(taskLocked({ ...state(), phase: "feedback" }), true);
});

test("every task error code in the contract has its own recovery action", () => {
  const contractText = read("docs/task-mode.md");
  const table = contractText.slice(contractText.indexOf("| Code "));
  const codes = [...table.matchAll(/^\| `([a-z_]+)`/gm)].map((row) => row[1]);
  assert.ok(codes.length > 15);
  // These are recovered by reloading; every other code names its own step.
  const reloads = new Set([
    "csrf_rejected",
    "client_override",
    "request_too_large",
  ]);
  for (const code of codes)
    if (!reloads.has(code)) assert.notEqual(taskRecovery(code), "Reload", code);
});

test("an ended attempt is described by its rule and time, never by intent", () => {
  assert.equal(
    outcomeSentence({
      outcome: "invalid",
      cause: "look_away",
      at: 192,
      durationMs: 9000,
    }),
    "Your face was out of frame or turned down at 03:12; the attempt ended under the announced rules and is invalid.",
  );
  assert.match(
    outcomeSentence({
      outcome: "interrupted",
      cause: "camera",
      at: 5,
      durationMs: 0,
    }),
    /interrupted and can be started again/,
  );
  assert.equal(
    outcomeSentence({
      outcome: "completed",
      cause: "finish",
      at: 600,
      durationMs: 0,
    }),
    "You finished the task.",
  );
  for (const cause of ["fullscreen", "visibility", "look_away"])
    assert.doesNotMatch(
      outcomeSentence({ outcome: "invalid", cause, at: 1, durationMs: 0 }),
      /cheat|dishonest/i,
    );
});

test("the rules read the same as the instructor's page states them", () => {
  const lines = rulesInWords({
    durationMin: 20,
    lookAwaySeconds: 9,
    closesAt: "2026-12-31T23:59:59Z",
    rulesNote: "Bring paper.",
  });
  assert.match(lines[0], /20 minutes/);
  assert.ok(lines.some((line) => /9 seconds/.test(line)));
  assert.ok(lines.some((line) => /calculator/.test(line)));
  assert.equal(
    lines.at(-2),
    "Attempts must start before 2026-12-31T23:59:59Z.",
  );
  assert.equal(lines.at(-1), "Bring paper.");
  // tests/test_task_package.py holds the packaging tool's wording to these
  // sentences in full.
});

test("task wire messages are closed and preserve original revision", () => {
  assert.deepEqual(taskActionPayload("a", "finish", "revision-1"), {
    version: 1,
    requestId: "a",
    action: "finish",
    revisionId: "revision-1",
    targetId: null,
  });
  assert.deepEqual(taskStartPayload(), { version: 1 });
  assert.deepEqual(taskEndPayload("e", "invalid", "look_away", 8000.4), {
    version: 1,
    requestId: "e",
    outcome: "invalid",
    cause: "look_away",
    durationMs: 8000,
  });
  assert.equal(
    taskRevisionPayload("r", "revision-1", "code", "run").revisionId,
    "revision-1",
  );
  assert.equal(
    taskRunPayload("result", "run", "revision-1", 1, 2, "").revisionId,
    "revision-1",
  );
});

test("the server task review fixtures have integer versions and render as sent", () => {
  const fixture = JSON.parse(read("tests/fixtures/task-review.json"));
  assert.equal(fixture.topic, "task_review");
  for (const { payload } of fixture.cases) {
    assert.equal(Number.isInteger(payload.taskAssessment.setVersion), true);
    assert.deepEqual(taskReviewForDisplay(payload), payload);
  }
});

test("every language a task may allow has a name and a file extension", () => {
  // The server's list is LANGUAGES in src/tasks/mod.rs; the runners' is
  // this one, and the two name the same five.
  assert.deepEqual(Object.keys(LANGUAGE_LABELS), [...ALL_LANGUAGES]);
  assert.deepEqual(
    ALL_LANGUAGES.map((language) => taskCodeFileName("two-sum", language)),
    ["two-sum.py", "two-sum.js", "two-sum.c", "two-sum.cpp", "two-sum.java"],
  );
});
