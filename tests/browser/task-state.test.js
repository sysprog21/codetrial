import test from "node:test";
import assert from "node:assert/strict";
import {
  attemptFinished,
  hasStarted,
  monitoring,
  st,
} from "../../web/task-state.js";

test("the page state is sealed, so a misspelled field throws on a write", () => {
  assert.equal(Object.isSealed(st), true);
  assert.throws(() => {
    st.atempt = "running";
  }, TypeError);
});

test("an attempt starts preparing, in Python, with nothing in flight", () => {
  assert.equal(st.attempt, "preparing");
  assert.equal(st.language, "python");
  assert.equal(st.revisionId, "initial");
  for (const flag of [
    "runBusy",
    "actionPending",
    "startUnconfirmed",
    "reviewPolling",
    "faceReady",
    "calibrating",
    "enteringFullscreen",
    "restored",
  ])
    assert.equal(st[flag], false, flag);
  for (const empty of [
    "builtIn",
    "serverClockOffset",
    "startViolation",
    "acknowledgedAt",
    "media",
    "detector",
    "calibration",
    "monitorTimer",
    "wakeLock",
    "beepContext",
    "startInFlight",
  ])
    assert.equal(st[empty], null, empty);
});

test("the fields the page computes from its URL are filled in by task.js", () => {
  for (const computed of ["setId", "taskId", "site", "version", "admissionKey"])
    assert.ok(computed in st, computed);
  assert.equal(st.setId, undefined, "task.js assigns it, not this module");
});

test("an attempt has started once anything but preparing is the state", () => {
  const saved = st.attempt;
  try {
    for (const [attempt, started] of [
      ["preparing", false],
      ["starting", true],
      ["running", true],
      ["ending", true],
      ["lost", true],
      ["done", true],
    ]) {
      st.attempt = attempt;
      assert.equal(hasStarted(), started, attempt);
    }
  } finally {
    st.attempt = saved;
  }
});

test("an attempt is finished when its result is shown or its room is lost", () => {
  const saved = st.attempt;
  try {
    for (const [attempt, finished] of [
      ["preparing", false],
      ["starting", false],
      ["running", false],
      ["ending", false],
      ["lost", true],
      ["done", true],
    ]) {
      st.attempt = attempt;
      assert.equal(attemptFinished(), finished, attempt);
    }
  } finally {
    st.attempt = saved;
  }
});

test("the rules are watched while the attempt runs and before feedback", () => {
  const saved = [st.attempt, st.state];
  try {
    st.attempt = "running";
    st.state = undefined;
    assert.equal(monitoring(), true, "no server state yet");
    st.state = { phase: "work" };
    assert.equal(monitoring(), true);
    st.state = { phase: "feedback" };
    assert.equal(monitoring(), false, "feedback ends the watching");
    st.attempt = "ending";
    st.state = { phase: "work" };
    assert.equal(monitoring(), false, "only a running attempt is watched");
  } finally {
    [st.attempt, st.state] = saved;
  }
});
