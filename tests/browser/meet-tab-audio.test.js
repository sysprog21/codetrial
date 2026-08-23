// Run with: node --test tests/browser/meet-tab-audio.test.js
//
// Google Meet mode is instructions plus an output selector. Both are text the
// candidate follows, so text is what these check. The one assertion that earns
// its keep is the getUserMedia allowlist: the whole point of this mode is that
// CodeTrial never becomes a microphone for Meet, and the cheapest way to keep
// it that way is to fail the build when a new capture call site appears.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { firstPartyScripts as webScripts, root } from "./source.js";
import { outputAfterRouting, outputOptions } from "../../web/meet-audio.js";

const web = join(root, "web");
const read = (name) => readFileSync(join(web, name), "utf8");
const page = read("interview.html");
const script = read("interview.js");

// vendor/ is third-party and legitimately contains these strings, so the shared
// helper excludes it by path. Recursive, so a bridge added under web/avatar/ or
// web/recording/ is covered rather than skipped by accident.
const firstPartyScripts = webScripts();

const steps = [
  "Start the CodeTrial interview and confirm the media preflight.",
  "In Meet, click Present now.",
  "Choose <strong>A tab</strong>.",
  "Select the CodeTrial tab.",
  "Enable <strong>Share tab audio</strong> before clicking Share.",
];

test("meet-mode instructions", () => {
  for (const step of steps) {
    assert.ok(page.includes(step), `interview.html must carry the step: ${step}`);
  }
  // The two choices Meet gets wrong by default. Without both, the interviewer
  // watches a silent tab and nobody finds out until the interview starts.
  assert.match(page, /Share <strong>A tab<\/strong>, not a window or your whole screen/);
  assert.match(page, /turn on\s+<strong>Share tab audio<\/strong>/);
  // The microphone cannot serve both, so the page must say where the
  // interviewer actually hears the candidate rather than implying Meet does.
  assert.match(page, /Your microphone stays with CodeTrial and cannot also serve Meet/);
  assert.match(page, /the interviewer\s+hears you by joining the interview room/);
  assert.match(page, /Google Meet controls who receives this interview/);
  assert.match(page, /outside CodeTrial's control/);
});

test("meet-mode dom ids", () => {
  const ids = new Set([...page.matchAll(/id="([^"]+)"/g)].map((match) => match[1]));
  // Only the ids no script queries. Everything reached by querySelector is
  // already covered generically by dom-contract.test.js, "every id a script
  // queries exists in the markup", so repeating them here proves nothing twice.
  for (const id of ["meet-mode-steps", "meet-mode-reminder"]) {
    assert.ok(ids.has(id), `interview.html must keep #${id}`);
  }
  // The five steps are an ordered list the candidate follows top to bottom.
  // Search for the close tag from the start offset, not from 0: an earlier
  // <ol> anywhere on the page would otherwise invert the slice.
  const start = page.indexOf('id="meet-mode-steps"');
  const list = page.slice(start, page.indexOf("</ol>", start));
  assert.equal([...list.matchAll(/<li>/g)].length, 5, "the mode documents exactly five steps");
});

// A bare "no getUserMedia" is false: the media preflight is mandatory and has
// to capture. So pin which FILES may capture, across every first-party script
// and not just this one. Matching the whole file rather than line by line
// catches a call wrapped across lines; the extra alternatives catch the obvious
// dodges. One site, because the preflight asks per device through a single
// request path: a second site means a new capture, not a second device.
test("meet-mode getusermedia allowlist", () => {
  const capture = /getUserMedia\s*\(|\[\s*["']getUserMedia["']\s*\]|getUserMedia\s*\.\s*(?:call|apply|bind)/g;
  const sites = firstPartyScripts.flatMap((name) =>
    [...read(name).matchAll(capture)].map(() => name));

  assert.deepEqual(sites, ["devices.js"],
    "a new getUserMedia call site appeared; Meet mode must not capture audio for Meet");
});

// These execute the decisions instead of reading them. The text assertions
// below still guard the wiring, but a wiring assertion cannot tell whether the
// logic is right, and both bugs this feature shipped with were logic.
test("sink routing offers only devices the browser has named", () => {
  const before = [{ kind: "audiooutput", deviceId: "", label: "" }];
  const after = [
    { kind: "audiooutput", deviceId: "speakers", label: "Built-in" },
    { kind: "audioinput", deviceId: "mic", label: "Mic" },
    { kind: "audiooutput", deviceId: "usb", label: "" },
  ];

  // Pre-grant placeholder: an empty deviceId renders as a real choice and then
  // routes to nothing, so it must not reach the list at all.
  assert.deepEqual(outputOptions(before, null), []);
  assert.deepEqual(outputOptions(after, "usb"), [
    { deviceId: "speakers", label: "Built-in", selected: false },
    { deviceId: "usb", label: "Output 2", selected: true },
  ]);
  // An unnamed device still gets a usable label, and inputs never appear.
  assert.deepEqual(
    outputOptions(after, null).map((option) => option.deviceId),
    ["speakers", "usb"],
  );
});

test("sink routing keeps the preference and the real sink in agreement", () => {
  // Success: the pick becomes the preference.
  assert.equal(outputAfterRouting("old", "new", true), "new");
  // Refusal rolls back. Keeping the refused device would leave the select
  // showing a speaker Jim never played through, until the next reload.
  assert.equal(outputAfterRouting("old", "new", false), "old");
  // Refused with nothing to fall back to: no preference at all, so the caller
  // clears the key rather than persisting the device that just failed.
  assert.equal(outputAfterRouting(null, "new", false), null);
});

// What the executable tests above cannot see: that interview.js actually wires
// those decisions to the right nodes and the right track.
test("sink routing", () => {
  // setSinkId targets Jim's element and nothing else, so the candidate cannot
  // accidentally reroute their own microphone monitoring.
  const sinkCalls = [...script.matchAll(/(\w+)\s*\n?\s*\.setSinkId\(/g)].map((match) => match[1]);
  assert.deepEqual(sinkCalls, ["jimAudio"], "setSinkId may only target Jim's audio element");
  assert.match(script, /element\.id = "jim-audio"/, "the remote audio element needs a stable id");
  assert.match(script, /outputOptions\(devices, readStored\(AUDIO_OUTPUT_KEY\)\)/, "the selector is built from the shared decision");
  // Persist before routing. A choice made during the preflight, before Jim's
  // track exists, is dropped if it only survives a successful setSinkId.
  const apply = script.slice(script.indexOf("async function applyAudioOutput"));
  assert.ok(
    apply.indexOf("writeStored(AUDIO_OUTPUT_KEY") < apply.indexOf(".setSinkId("),
    "applyAudioOutput must persist before it routes",
  );
  assert.match(apply, /outputAfterRouting\(previous, deviceId, routed\)/, "rollback comes from the tested decision");
  // One element for the session, and it is released on unsubscribe.
  assert.match(script, /jimAudio \?\? document\.createElement\("audio"\)/, "reuse one audio element");
  assert.match(script, /TrackUnsubscribed, dropRemoteAudio/, "unsubscribed audio must be detached");
  // Unsupported browsers get told, not silently downgraded, and the whole row
  // hides so the label does not dangle over a control that is gone.
  assert.match(script, /"setSinkId" in HTMLMediaElement\.prototype/);
  assert.match(script, /meetOutputRow\.hidden = true/);
  assert.match(script, /showOutputNote\(OUTPUT_NOTES\.unsupported\)/);
});

test("no synthetic microphone", () => {
  const offenders = firstPartyScripts.filter((name) =>
    /createMediaStreamDestination|getDisplayMedia/.test(read(name)));

  assert.deepEqual(offenders, [], "CodeTrial must not build an audio bridge or capture the screen for Meet");
});

// Meet cannot open a camera CodeTrial is holding, so the release has to be
// real: stopped AND out of the stream. Removing it from the stream is what
// makes every downstream reader (publisher, watchers, poller, heartbeat) do
// the right thing through its existing null guard, instead of each one needing
// to know about a mode flag.
test("meet presentation releases the camera without accusing the candidate", () => {
  assert.match(page, /id="meet-presentation"/, "the mode is chosen in the preflight, before the room starts");
  assert.match(script, /camera\.stop\(\)/, "the device is released, not merely unwatched");
  assert.match(script, /stream\.removeTrack\?\.\(camera\)/, "a stopped track left in the stream is still a track every reader sees");

  // Released before connect(), so nothing downstream is ever handed a camera
  // that is about to vanish.
  const release = script.indexOf("releaseCameraToPresenter(preflight.userStream)");
  const joinsRoom = script.indexOf("await connect(preflight");
  assert.notEqual(release, -1, "presentation mode must release the camera");
  assert.notEqual(joinsRoom, -1, "init must still join the room");
  assert.ok(release < joinsRoom, "release before the room, or the publisher still sees the track");

  // The mode must not leak back into the media pipeline as special cases.
  // These four all read the stream and get the right answer for free once the
  // track is gone; a flag check reappearing here means the fix regressed.
  for (const fn of ["publishPreflightTracks", "monitorIntegrityTracks", "startIntegrityTrackStateMonitor", "integrityHeartbeatDetail"]) {
    const start = script.indexOf(`function ${fn}`);
    assert.notEqual(start, -1, `${fn} should still exist`);
    const body = script.slice(start, script.indexOf("\nfunction ", start + 1));
    assert.doesNotMatch(body, /meetPresentation/, `${fn} must read the stream, not a mode flag`);
  }

  // `presenting` was read inside connectLiveKit while only connect had it in
  // scope. Every interview threw `ReferenceError: presenting is not defined`
  // right after joining the room, fell into the offline-practice catch, and
  // nulled state.room while the real LiveKit room stayed connected underneath:
  // Jim greeted the candidate and then nothing worked. node --check cannot see
  // this and no node test can execute this file, so the scope is pinned as
  // text. A tripwire, not a proof.
  assert.match(script, /async function connectLiveKit\(connection, preflight, presenting = false\)/,
    "connectLiveKit reads `presenting`, so it must receive it");
  assert.match(script, /await connectLiveKit\(connection, preflight, presenting\)/,
    "and the caller must pass it");
  // The release is evidence, not silence: a report with no face events must not
  // read like one where the camera watched and saw nothing.
  assert.match(script, /CAMERA_RELEASED_TO_PRESENTER/, "the release is recorded for human review");
  // Keyed on the candidate's choice, not on an empty stream. A camera that
  // died between the preflight and the room leaves the stream empty too, and
  // recording that as a deliberate release puts a claim in the signed trail
  // that the candidate never made.
  const connectFn = script.slice(script.indexOf("async function connect("));
  const branch = connectFn.slice(connectFn.indexOf("startIntegrityHeartbeat()"));
  assert.match(branch, /if \(presenting\)/, "the release event must key on intent");
  assert.doesNotMatch(
    branch.slice(0, branch.indexOf("CAMERA_RELEASED_TO_PRESENTER")),
    /getVideoTracks/,
    "an empty stream is not consent to record a release",
  );
  assert.match(
    read("../src/agent/integrity.rs"),
    /"CAMERA_RELEASED_TO_PRESENTER" => "CAMERA_RELEASED_TO_PRESENTER"/,
    "the server must accept the event type, or the evidence is dropped",
  );
});

// Both device requests failing used to schedule a retry each, and both retries
// called back into the same function, so a denied prompt doubled the number of
// in-flight getUserMedia calls every two seconds until the tab ran out of memory.
// `devices.test.js` drives the collapsing itself, by denying both prompts and
// counting the requests that follow. What is left here is the shape a counting
// test cannot see: that no second path schedules a start behind the timer's
// back, which is how the doubling bug arrived in the first place.
test("the media preflight retries through a single shared timer", () => {
  const source = read("devices.js");
  const scheduled = [...source.matchAll(/\bschedule\(/g)];
  assert.equal(scheduled.length, 1,
    "the start must be scheduled only by retry(), which collapses concurrent retries");
  assert.match(source, /retryTimer \?\?= schedule\(/,
    "the shared retry timer must not be replaced by a fresh one while it is pending");
  assert.match(read("interview.js"), /pool\.cancelRetry\(\)/,
    "finishing the preflight must cancel the pending retry");
});
