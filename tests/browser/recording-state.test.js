// Run with: node --test tests/browser/recording-state.test.js
//
// The recording label is a promise to a person: the candidate agreed to be
// recorded and is owed the answer to "is it". This runs `web/recording-state.js`
// against stub timers and a stub fetch, so what it asserts is behaviour --
// which answers arm a poll, which stop it, and that exactly one poll is ever
// live -- rather than the text of the functions.

import { test } from "node:test";
import assert from "node:assert/strict";

import { failFetchWith } from "./source.js";
import {
  initRecording,
  pollRecordingState,
  setRecordingPoll,
  showRecordingState,
  startRecording,
} from "../../web/recording-state.js";

function response(status, body = {}) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

/// Stub timers, fetch and the one node this module writes to.
///
/// `recordingPoll` is closure-private, so no test can read it. `armed` and
/// `cleared` are how a test sees it instead: a handle that was armed and never
/// cleared is a poll still running, which is the failure these exist to catch
/// and the one nothing else in the tree would notice.
function harness(t, fetchStub) {
  const armed = [];
  const cleared = [];
  const restoreFetch = failFetchWith(fetchStub);
  const savedInterval = globalThis.setInterval;
  const savedClear = globalThis.clearInterval;
  const savedWarn = console.warn;
  // The handle is the position, counting from one, so a test can name the
  // interval a particular attempt armed rather than count clears.
  globalThis.setInterval = (callback, delay) => armed.push(delay);
  globalThis.clearInterval = (handle) => void cleared.push(handle);
  console.warn = () => {};
  const nodes = { recordingState: { hidden: true, textContent: "" } };
  initRecording({ state: { interviewId: "interview/one" }, nodes, recordingEnabled: true });
  t.after(() => {
    setRecordingPoll(false);
    restoreFetch();
    globalThis.setInterval = savedInterval;
    globalThis.clearInterval = savedClear;
    console.warn = savedWarn;
  });
  return { nodes, armed, cleared };
}

/// The verdict belongs to the attempt that earned it.
///
/// A terminal answer used to be left on the node, where the next attempt read
/// it as its own and never polled the one route that could have established
/// whether that attempt started. A refused start is not a verdict: a start
/// that worked and could not be read back looks identical from here.
test("a start after a terminal answer polls again", async (t) => {
  const { nodes, armed } = harness(t, async () => response(200, { state: "ready" }));

  await startRecording();
  assert.deepEqual(armed, [], "a recording already saved is not asked about");

  globalThis.fetch = async () => response(503, { error: "temporarily unavailable" });
  await startRecording();
  assert.equal(nodes.recordingState.textContent, "Checking whether the recording started.");
  assert.deepEqual(armed, [15_000], "the attempt that could not be read back must be polled for");
});

/// One live poll, whatever the overlap.
///
/// The request sits between the two, so two attempts can both be inside
/// `startRecording` before either has an answer. A newer terminal answer must
/// keep its verdict when the older nonterminal answer eventually arrives.
test("a stale start cannot rearm a terminal recording poll", async (t) => {
  let releaseFirst;
  const firstAnswer = new Promise((resolve) => {
    releaseFirst = resolve;
  });
  let calls = 0;
  const { armed, cleared } = harness(t, async () => {
    if ((calls += 1) === 1) {
      await firstAnswer;
      return response(200, { state: "starting" });
    }
    return response(200, { state: "ready" });
  });

  const first = startRecording();
  const second = startRecording();
  await second;
  releaseFirst();
  await first;

  assert.deepEqual(armed, [], "an older nonterminal answer cannot restart polling");
  assert.ok(cleared.includes(null), "the terminal answer stops the current poll");
});

/// One live poll, arming included.
///
/// The generation guard settles which overlapping attempt owns the answer, but
/// a second attempt that starts after the first has finished is current in its
/// own right and reaches the arming with a poll already running. Clearing and
/// arming in one statement is what stops that from leaving an interval with
/// nothing holding its handle.
test("arming a poll clears the one already running", async (t) => {
  const { armed, cleared } = harness(t, async () => response(503, { error: "unavailable" }));

  await startRecording();
  assert.deepEqual(armed, [15_000], "the first attempt polls");

  await startRecording();

  assert.deepEqual(armed, [15_000, 15_000], "so does the second");
  assert.ok(cleared.includes(1), "and the first attempt's interval must not be left running");
});

/// A poll belongs to the attempt that armed it.
///
/// The request sits across a start, so an answer fetched for the previous
/// attempt can land after a newer one has taken over the timer. Applying it
/// would label the interview from a request that is no longer about it, and a
/// terminal one would stop the poll the current attempt is relying on.
test("a poll from a previous attempt cannot stop the current one", async (t) => {
  let releasePoll;
  const pollAnswer = new Promise((resolve) => {
    releasePoll = resolve;
  });
  let calls = 0;
  const { nodes, armed, cleared } = harness(t, async () => {
    calls += 1;
    if (calls === 2) {
      await pollAnswer;
      return response(200, { state: "ready" });
    }
    return response(200, { state: "starting" });
  });

  await startRecording();
  const stale = pollRecordingState();
  await startRecording();
  assert.deepEqual(armed, [15_000, 15_000], "the newer attempt armed its own poll");

  releasePoll();
  await stale;

  assert.equal(
    nodes.recordingState.textContent,
    "Recording is starting.",
    "a stale answer must not relabel the interview",
  );
  assert.ok(!cleared.includes(2), "nor stop the poll the current attempt is relying on");
});

/// The body is a second await, and the generation can move across it.
///
/// Checking once when the headers arrive is not enough: the answer is still
/// being parsed, and a start that lands in that window takes the timer over
/// before the verdict is applied to it.
test("a poll answer parsed after a newer start is discarded", async (t) => {
  let releaseBody;
  let reachedBody;
  const body = new Promise((resolve) => {
    releaseBody = resolve;
  });
  const parsing = new Promise((resolve) => {
    reachedBody = resolve;
  });
  let calls = 0;
  const { nodes, cleared } = harness(t, async () => {
    calls += 1;
    if (calls === 2) {
      return {
        ok: true,
        status: 200,
        json: () => {
          reachedBody();
          return body;
        },
      };
    }
    return response(200, { state: "starting" });
  });

  await startRecording();
  const stale = pollRecordingState();
  await parsing;
  await startRecording();

  releaseBody({ state: "ready" });
  await stale;

  assert.equal(
    nodes.recordingState.textContent,
    "Recording is starting.",
    "a body parsed after a newer start must not relabel the interview",
  );
  assert.ok(!cleared.includes(2), "nor stop the poll that start armed");
});

/// Gone, not ours, or signed out: none of them change by asking again.
test("a status route that has nothing to say stops the poll", async (t) => {
  const { nodes, cleared } = harness(t, async () => response(200, { state: "starting" }));

  await startRecording();
  globalThis.fetch = async () => response(404);
  await pollRecordingState();

  assert.equal(
    nodes.recordingState.textContent,
    "The recording did not start. The interview is not being recorded.",
  );
  // The armed handle by name, not `cleared.length`: the clear is
  // unconditional, so a count is satisfied by clearing nothing.
  assert.ok(cleared.includes(1), "the handle that was armed is the handle that is cleared");
});

/// Terminal on the answer's own terms, not on the status code.
///
/// `ready` arrives with a 200, so the route cannot be what stops the poll; the
/// state in the body has to, through the verdict `showRecordingState` reports.
test("a terminal state stops the poll and says so to its caller", async (t) => {
  const { cleared } = harness(t, async () => response(200, { state: "starting" }));

  await startRecording();
  assert.equal(showRecordingState({ state: "ready" }), true, "a saved recording is terminal");
  globalThis.fetch = async () => response(200, { state: "ready" });
  await pollRecordingState();

  assert.ok(cleared.includes(1), "nothing keeps asking after a terminal state");
});

/// Stopped, not just abandoned.
///
/// Clearing the interval and keeping the handle would let the next stop clear
/// a timer id the platform has already reused.
test("the poll drops the handle it cleared", async (t) => {
  const { cleared } = harness(t, async () => response(200, { state: "starting" }));

  await startRecording();
  setRecordingPoll(false);
  setRecordingPoll(false);

  assert.deepEqual(cleared.slice(-2), [1, null], "cleared once, then there is nothing to clear");
});
