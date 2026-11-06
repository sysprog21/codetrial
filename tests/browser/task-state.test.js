import test from "node:test";
import assert from "node:assert/strict";
import { st } from "../../web/task-state.js";

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
