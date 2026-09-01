// Run with: node --test tests/browser/recording-template.test.js
//
// The recording template is the one page in this repo nobody looks at while it
// runs: Egress opens it in a headless Chrome and the first evidence a person
// gets is an MP4 of whatever it rendered. Nothing here can execute it, so these
// read the page and its script as text, the same tripwire bargain the other
// browser tests make.
//
// What actually proves the template works is task 7a's credentialed Egress run
// against a real room. These keep the layout, the readiness rule and the
// participant rules from drifting between such runs.

import { test } from "node:test";
import assert from "node:assert/strict";
import { functionBody, read, interviewSource } from "./source.js";

const interview = withoutInterviewComments(interviewSource());
const page = read("web/recording/index.html");
const script = read("web/recording/recording.js");
const styles = read("web/recording/recording.css");

const withoutComments = (source) => source.replace(/^\s*\/\/.*$/gm, "");
function withoutInterviewComments(source) {
  return source.replace(/^\s*\/\/.*$/gm, "").replace(/^\s*\/\/\/.*$/gm, "");
}
const code = withoutComments(script);

test("recording-template dom ids", () => {
  // Every id the fixture, the CSS and the bootstrap in 8b all agree on. A
  // layout is free to change; these names are not, because the recorder cannot
  // report a selector that stopped matching.
  const required = [
    "stage",
    "problem-title",
    "problem-meta",
    "code",
    "tests",
    "timer",
    "candidate-video",
    "jim-panel",
    "jim-avatar",
    "jim-state",
    "recording-ready",
  ];
  for (const id of required) {
    assert.ok(page.includes(`id="${id}"`), `the template needs #${id}`);
  }

  // Queried, for the nodes this file writes to. The problem, code, test and
  // timer panels are in the page and stay empty until the replay bootstrap
  // fills them, and a lookup for a node nothing writes would read as wiring.
  for (const id of ["stage", "candidate-video", "jim-state", "recording-ready"]) {
    assert.ok(
      code.includes(`querySelector("#${id}")`),
      `#${id} is in the page but nothing in the script reads it`,
    );
  }

  assert.ok(
    /<video id="candidate-video"[^>]*muted/.test(page),
    "the camera element must be muted, or the room's audio is recorded twice",
  );
  assert.ok(
    !/id="(problem-title|timer)"[^>]*>[^<]/.test(page),
    "the panels start empty; a placeholder is a frame of the recording saying something untrue",
  );

  // Every voice, not only the interviewer's. Egress records what the page
  // plays, so a subscribed audio track that is never attached is a candidate
  // answering questions nobody can hear them answer.
  const connect = withoutComments(functionBody(script, "connect"));
  assert.ok(
    connect.includes('if (track.kind === "audio" && !isRecorder(participant))'),
    "audio is attached for everyone in the room except the recorder",
  );
  assert.ok(
    /<video id="candidate-video"[^>]*playsinline/.test(page),
    "playsinline, or the recorder gets a native player rather than the layout",
  );
  assert.ok(
    page.includes('href="/recording/recording.css"') &&
      page.includes('src="/recording/recording.js"'),
    "the template loads its own stylesheet and script",
  );
  assert.ok(
    page.includes('src="/vendor/livekit-client.js"'),
    "the SDK is vendored; a template that fetched it from a CDN would be blocked by the policy",
  );
  assert.ok(
    !/@media/.test(styles),
    "the recorder's viewport is one fixed shape, so a media query here only ever fires wrong",
  );
});

test("recording-template readiness", () => {
  assert.ok(
    /id="recording-ready" data-ready="false" hidden/.test(page),
    "the marker starts not ready, because the page starts empty",
  );

  const body = withoutComments(functionBody(script, "markReady"));
  assert.ok(
    body.includes("if (started || !cameraAttached || !bootstrapped) return;"),
    "the marker moves once, and only when there is something worth recording",
  );
  assert.ok(body.includes('nodes.ready.dataset.ready = "true";'));
  // The console, not a callback. Egress watches Chrome's console output for
  // these two exact strings, so a page that called an injected function would
  // never start the recording it was loaded for.
  assert.ok(
    body.includes('console.log("START_RECORDING")'),
    "the recorder is told, through the console, when the page is worth recording",
  );
  assert.ok(
    !/window\.START_RECORDING/.test(code),
    "there is no injected callback to wait for",
  );

  // The rule that matters: readiness follows the candidate's camera being
  // attached, not merely the room being joined. A frame recorded before that is
  // a frame of an empty layout, and nothing downstream can tell.
  const attach = withoutComments(functionBody(script, "attachCandidate"));
  assert.ok(attach.includes("track.attach(nodes.candidateVideo)"));
  assert.ok(attach.includes("markReady()"));

  // One way in. A subscription handler that attached whatever camera arrived
  // would make "the candidate" mean "whoever published video first".
  const subscribed = withoutComments(functionBody(script, "connect"));
  assert.ok(
    !/attachCandidate\(/.test(subscribed),
    "the subscription handler goes through the selector, not straight to the element",
  );
  assert.ok(subscribed.includes("refreshCandidate(room)"));
  assert.ok(
    !/markReady\(\)/.test(withoutComments(functionBody(script, "connect"))),
    "connecting is not readiness",
  );

  assert.ok(
    code.includes('console.log("END_RECORDING")'),
    "a room that ended must not leave the job running until the provider's timeout",
  );
  const gone = withoutComments(functionBody(script, "connect"));
  assert.ok(
    /track\.detach\(\)/.test(gone),
    "a candidate who left must not be recorded as a frozen last frame",
  );
  assert.ok(
    /element === nodes\.candidateVideo/.test(gone),
    "the camera element is the layout's: removing it leaves a rejoining candidate nothing to attach to",
  );
  assert.ok(
    /cameraAttached = false;/.test(gone),
    "and a camera that left before readiness must not still count as one",
  );
  assert.ok(
    /cameraAttached = false;\s*[\s\S]{0,400}?refreshCandidate\(room\);/.test(gone),
    "then ask again: a republished camera is already on that element and the old track's detach clears it",
  );
});

test("recording-bootstrap snapshot-first", () => {
  const body = withoutComments(functionBody(script, "connect"));
  const snapshot = body.indexOf("await pumpReplay()");
  const loop = body.indexOf("schedulePoll()");
  assert.ok(snapshot !== -1, "the template asks for the replay");
  assert.ok(loop !== -1, "and keeps asking");
  assert.ok(
    snapshot < loop,
    "the snapshot is awaited before the loop starts, or a tail can land while it is still in flight",
  );

  // A timer scheduled after the answer, not an interval with a busy flag: the
  // second is the first with a way to get it wrong.
  const poll = withoutComments(functionBody(script, "schedulePoll"));
  assert.ok(poll.includes("if (replayClosed) return;"));
  assert.ok(poll.includes("setTimeout(() => void pumpReplay().then(schedulePoll)"));
  assert.ok(!/setInterval/.test(code), "no interval can outpace a slow answer");

  const pump = withoutComments(functionBody(script, "pumpReplay"));
  assert.ok(
    pump.includes('const query = lastSeq < 0 ? "" : `?after=${lastSeq}`;'),
    "the first call asks for a snapshot and every later one asks for what came after",
  );
  assert.ok(
    pump.includes("headers: { authorization: token || \"\" }"),
    "the room token is the credential; this page has no cookie to send",
  );
  assert.ok(
    /\/api\/recording\/replay/.test(pump),
    "and it reads the route that authorizes by room rather than by account",
  );
});

test("recording-bootstrap late-join", () => {
  // A recorder that starts after the interview began gets the state of the
  // interview, not the keystrokes that produced it. That is what the snapshot
  // is, and this page must not paint before it has one.
  const ready = withoutComments(functionBody(script, "markReady"));
  assert.ok(
    ready.includes("!cameraAttached || !bootstrapped"),
    "readiness waits for both the camera and the replay",
  );

  const pump = withoutComments(functionBody(script, "pumpReplay"));
  assert.ok(
    /replayClosed = true;\s*bootstrapped = true;/.test(pump),
    "a refused or expired replay still lets the recording start; the camera is worth the file",
  );
  assert.ok(
    /catch \{[^}]*giveUpEventually\(\);/.test(pump),
    "a dropped request does not count as a snapshot",
  );
  assert.ok(
    pump.includes("if (response.ok) body = await response.json();"),
    "the body is read inside the same guard, or a truncated answer ends the polling",
  );
  assert.ok(
    pump.includes("signal: AbortSignal.timeout(REPLAY_TIMEOUT_MS)"),
    "a request that hangs answers neither way, and the first read is what the recording waits on",
  );
  const giveUp = withoutComments(functionBody(script, "giveUpEventually"));
  assert.ok(
    giveUp.includes("if (Date.now() - openedAt < REPLAY_BOOTSTRAP_MS) return;"),
    "but a replay that never answers must not hold the recording forever either",
  );
  assert.ok(
    !/replayAttempts/.test(code),
    "measured on the clock, not counted in attempts: each attempt can burn its whole timeout",
  );
  assert.ok(giveUp.includes("bootstrapped = true;"));
  assert.ok(
    /const RETRY_NEVER = \[401, 403, 404, 410\];/.test(code),
    "a 500 or a dropped connection is a bad moment, not an answer",
  );
  assert.ok(
    /catch \{[\s\S]*?\}/.test(pump),
    "a dropped request is retried rather than treated as a refusal",
  );
  assert.ok(
    pump.includes("lastSeq = Math.max(lastSeq, body.seq)"),
    "the cursor never goes backwards, or a slow response would replay what was already applied",
  );
  assert.ok(
    !/body\?\.seq !== "number"[\s\S]{0,200}seq >= 0/.test(pump),
    "an empty replay answers seq -1 and is a perfectly good snapshot; refusing it would never bootstrap",
  );
});

test("recording-bootstrap ordering", () => {
  const pump = withoutComments(functionBody(script, "pumpReplay"));
  assert.ok(
    pump.includes("for (const event of body.events) applyReplayEvent(event);"),
    "events are applied in the order they arrive, which is the order the server allocated",
  );
  assert.ok(
    /try \{\s*for \(const event of body\.events/.test(pump),
    "and a body this page did not expect cannot end the polling by throwing",
  );
  assert.ok(
    pump.includes('if (!Array.isArray(body?.events) || typeof body?.seq !== "number")'),
    "a 200 without the two fields this route always sends came from something else",
  );
  assert.ok(
    !/sort\(/.test(pump),
    "no client-side reordering: the sequence is the server's answer, not a hint",
  );

  const apply = withoutComments(functionBody(script, "applyReplayEvent"));
  for (const kind of ["stage", "editor", "tests", "avatar"]) {
    assert.ok(apply.includes(`case "${kind}":`), `the layout renders ${kind} events`);
  }
  assert.ok(
    apply.includes("default:"),
    "an unknown kind is ignored rather than refused: a later deploy's producer is not a reason to stop",
  );
  assert.ok(
    !/innerHTML/.test(code),
    "the candidate's own code is rendered as text; markup in it would be markup in the recording",
  );
  assert.ok(
    apply.includes("nodes.code.textContent = payload.code"),
    "and the editor panel is written as text",
  );
});

// The producers. Six kinds, six call sites, all of them in `web/interview.js`:
// the candidate's page is the only one that knows what the candidate is looking
// at, and the recording template renders what it sends.
//
// One assertion these all rest on: every producer goes through `recordReplay`,
// so "is this server recording" and "has the server heard enough" are answered
// once rather than at six call sites.
test("replay-producer envelope", () => {
  const record = withoutComments(functionBody(interview, "recordReplay"));
  assert.ok(
    record.includes("if (!recordingEnabled || !state.interviewId || replayClosed) return;"),
    "a server that records nothing, an interview that does not exist yet, and a replay the server has stopped accepting",
  );
  assert.ok(
    record.includes("replayQueue.push({ v: replayVersion, kind, at: Date.now(), payload });"),
    "one envelope, written once",
  );
  assert.ok(
    /globalThis\.CODETRIAL_REPLAY_VERSION/.test(interview),
    "the version comes from the server, because a second spelling of it refuses every event",
  );

  // The batch send lives in `sendQueuedBatch`; `flushReplay` is the queue in
  // front of it that keeps two POSTs from committing out of order.
  const flush = withoutComments(functionBody(interview, "sendQueuedBatch"));
  assert.ok(
    flush.includes("replayQueue.splice(0, REPLAY_MAX_BATCH)"),
    "batched to the server's own limit",
  );
  const chain = withoutComments(functionBody(interview, "flushReplay"));
  assert.ok(
    chain.includes("flushChain = flushChain.then(sendQueuedBatch);"),
    "flushes are serialized, or the replay is ordered by whichever request finished first",
  );
  assert.ok(
    flush.includes("if (response.status === 404) {") && flush.includes("closeReplay();"),
    "consent withdrawn: the server will refuse everything after this",
  );
  assert.ok(
    flush.includes('if (code === "replay_quota_exceeded") closeReplay();'),
    "and quota, which is the only 413 that is about the interview rather than the batch",
  );
  assert.ok(
    !/status === 413\s*\|\|/.test(flush) && !/\|\|\s*response\.status === 413/.test(flush),
    "a `replay_batch_too_large` must not close the feed: one oversized paste would end the replay",
  );
  const close = withoutComments(functionBody(interview, "closeReplay"));
  assert.ok(close.includes("replayClosed = true;"));
  assert.ok(
    close.includes("replayQueue = [];"),
    "and what is queued is dropped, not flushed: it is from before the decision that says not to store it",
  );
  const withdraw = withoutComments(functionBody(interview, "withdrawRecordingConsent"));
  assert.ok(
    withdraw.includes("closeReplay();"),
    "a candidate who has just said stop keeps sending nothing while the answer comes back",
  );
  assert.ok(
    /catch \{[\s\S]*?\}/.test(flush),
    "an offline moment does not take the interview down with it",
  );
});

test("replay-producer transcript", () => {
  const body = withoutComments(functionBody(interview, "consumeTranscript"));
  assert.ok(
    body.includes('recordReplay("transcript", { speaker, text: text.trim() });'),
    "the transcript is recorded as spoken turns",
  );
  const emits = body.match(/recordReplay\("transcript"/g) || [];
  assert.equal(emits.length, 1, "one emit, so a second inside the chunk loop cannot hide behind this one");
  assert.ok(
    body.indexOf('recordReplay("transcript"') > body.indexOf("if (text.trim())"),
    "once per turn, after the stream, not once per chunk",
  );
  assert.ok(
    body.indexOf("for await (const chunk of reader)") < body.indexOf("if (text.trim())"),
    "and the chunk loop is over by then",
  );
});

test("replay-producer editor", () => {
  const body = withoutComments(functionBody(interview, "flushPendingCodePublish"));
  assert.ok(
    body.includes('recordReplay("editor", { code: currentCode(), language: state.language });'),
    "the editor is recorded where the debounced publish already happens",
  );
  assert.equal(
    (interview.match(/recordReplay\("editor"/g) || []).length,
    2,
    "exactly two: the starter snapshot at the start and the debounced one, and nothing per keystroke",
  );
});

test("replay-producer tests", () => {
  const body = withoutComments(functionBody(interview, "runTests"));
  assert.ok(body.includes('recordReplay("tests", {'), "a run is recorded");
  assert.ok(
    body.includes("failed: Math.max(0, summary.total - summary.passed),"),
    "and the failures are derived rather than assumed present on the summary",
  );
  assert.ok(
    body.indexOf('recordReplay("tests"') > body.indexOf("renderResults(summary)"),
    "after the results are known",
  );
});

test("replay-producer stage", () => {
  const payload = withoutComments(functionBody(interview, "stagePayload"));
  assert.ok(
    payload.includes("title: problem.title, meta: nodes.meta.textContent"),
    "the problem heading is assembled in one place",
  );
  const stage = withoutComments(functionBody(interview, "recordStage"));
  assert.ok(
    stage.includes('recordReplay("stage", stagePayload());'),
    "the problem heading is sent, not assembled twice",
  );
  const problem = withoutComments(functionBody(interview, "renderProblem"));
  assert.ok(problem.includes("recordStage();"), "sent when the problem is rendered");
  const connect = withoutComments(functionBody(interview, "connect"));
  assert.ok(
    connect.includes("recordStage();"),
    "and again once the interview exists: the lobby render had nothing to attach it to",
  );
  assert.ok(
    connect.includes('recordReplay("editor", { code: currentCode(), language: state.language });'),
    "with the starter code, so a candidate who never types is not recorded beside an empty editor",
  );

  const timer = withoutComments(functionBody(interview, "tickTimer"));
  assert.ok(
    timer.includes("recordStageTick(tick.remaining);"),
    "the clock is restated on an interval, not on every tick",
  );
  const stageTick = withoutComments(functionBody(interview, "recordStageTick"));
  assert.ok(
    stageTick.includes("Date.now() - replayStageAt < REPLAY_STAGE_MS"),
    "the stage repeat must still be throttled, wherever the guard lives",
  );
  // Through `stagePayload`, so the tick carries the heading too. `stage` is a
  // snapshot kind: the newest one supersedes the rest, so a tick that sent only
  // the seconds would blank the problem title for a late replay reader.
  assert.ok(
    stageTick.includes('recordReplay("stage", stagePayload({ remainingSeconds }));'),
    "every stage event carries the heading, or the newest one erases it",
  );
  assert.ok(
    /const REPLAY_STAGE_MS = 15000;/.test(interview),
    "a second-by-second clock would be most of the per-interview budget",
  );
});

test("replay-producer avatar", () => {
  assert.ok(
    withoutComments(functionBody(interview, "recordAvatarState")).includes(
      'recordReplay("avatar", { state: value });',
    ),
    "the interviewer's state is recorded",
  );
  assert.ok(
    withoutComments(functionBody(interview, "recordAvatarState")).includes(
      "if (value === replayAvatarState) return;",
    ),
    "on the change, not on the participant event that happened to carry it",
  );
});

test("replay-producer lifecycle", () => {
  assert.ok(
    interview.includes('recordReplay("lifecycle", { state: "started", interviewLoop, codingMinutes, behavioralMinutes });'),
    "a template that joins late has to know the interview was already running",
  );
  const ending = withoutComments(functionBody(interview, "endInterview"));
  assert.ok(ending.includes('recordReplay("lifecycle", { state: "ended", reason });'));
  assert.ok(
    ending.includes("void flushReplay();"),
    "sent rather than queued: the page is about to stop flushing timers",
  );
  assert.ok(
    ending.indexOf('recordReplay("lifecycle"') < ending.indexOf("void flushReplay();"),
    "and queued before it is flushed, or the last event never leaves",
  );
});

test("recording-template identity rules", () => {
  // The agent is recognised the same way the interview page recognises it. Two
  // spellings of "is this the interviewer" is one deploy away from a recording
  // that treats Jim as the candidate.
  assert.ok(
    /import \{ isAgent \} from "\/lib\.js"/.test(script),
    "the agent rule is imported, not restated",
  );

  const candidate = withoutComments(functionBody(script, "findCandidate"));
  assert.ok(candidate.includes("!isAgent(participant)"));
  assert.ok(candidate.includes("!isRecorder(participant)"));
  assert.ok(
    candidate.includes("publishesCamera(participant)"),
    "the candidate is the one publishing a camera, which is what makes this by elimination",
  );

  const camera = withoutComments(functionBody(script, "publishesCamera"));
  assert.ok(
    camera.includes("window.LivekitClient.Track.Source.Camera"),
    "source, not kind: a screen share is video too, and it is not a face",
  );

  const recorder = withoutComments(functionBody(script, "isRecorder"));
  assert.ok(recorder.includes("permissions?.hidden"));
  assert.ok(recorder.includes('identity?.startsWith("EG_")'));

  // The credential comes from the query string and nothing here reasons about
  // its lifetime: Egress mints the join token, CodeTrial does not.
  assert.ok(code.includes('params.get("url")'));
  assert.ok(code.includes('params.get("token")'));
  assert.ok(code.includes('params.get("layout")'));
  assert.ok(
    !/expir|ttl|lifetime/i.test(code),
    "the template asserts nothing about a token it did not mint",
  );

  assert.ok(
    code.includes("adaptiveStream: false"),
    "adaptive streaming sizes a subscription to the element, which for a recorder is a blurry file",
  );
});
