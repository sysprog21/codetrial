// Run with: node --test tests/browser/avatar.test.js
//
// Two kinds of test here, deliberately separated. The behavior tests drive
// createAvatar with a fake clock, a fake mount, and a fake model, so they
// execute the real transitions rather than matching source text. The wiring
// tests read web/*.js and web/*.html, because a plain node run has no DOM and
// no WebGL and cannot observe rendering at all; those assertions are tripwires
// and are written to say so.

import { test } from "node:test";
import assert from "node:assert/strict";
import { captures, firstPartyScripts, functionBody, read, root } from "./source.js";
import { readdirSync } from "node:fs";
import { join } from "node:path";

import {
  ANALYSER_WINDOW,
  EXPRESSION_NAMES,
  BLINK_DURATION_MS,
  BLINK_INTERVAL_MS,
  BREATH_AMPLITUDE,
  GAZE_BOUND,
  HEAD_TILT_BOUND,
  MOUTH_FULL_AMPLITUDE,
  MOUTH_OPEN_THRESHOLD,
  TRANSITION_MS,
  blinkWeight,
  breathOffset,
  createAvatar,
  expressionForState,
  gazeForState,
  mouthFromAmplitude,
} from "../../web/avatar/avatar.js";

const page = read("web/interview.html");
const script = read("web/interview.js");

// A mount is a dataset and nothing else, which is the whole reason avatar.js
// may not reach for anything richer.
const fakeMount = () => ({ dataset: {} });

function fakeModel() {
  const poses = [];
  return { poses, disposed: false, apply(pose) { poses.push(pose); }, dispose() { this.disposed = true; } };
}

async function readyAvatar(options = {}) {
  const model = fakeModel();
  const mount = fakeMount();
  const avatar = createAvatar({ mount, loadModel: async () => model, now: () => 0, ...options });
  await avatar.ready;
  return { avatar, model, mount };
}

test("avatar vendor manifest pins every redistributed file", () => {
  const sums = read("web/vendor/avatar/SHA256SUMS");
  // Bare filenames, matching web/vendor/face-detection/SHA256SUMS, because
  // scripts/verify-vendor.sh runs shasum from each directory.
  assert.match(sums, /^[0-9a-f]{64} {2}three-vrm\.js$/m);
  assert.doesNotMatch(sums, /\//, "SHA256SUMS entries must be bare filenames");

  // Every redistributed file is either hashed or is its own license, and the
  // licenses are actually present rather than merely cited.
  const vendored = readdirSync(join(root, "web/vendor/avatar"));
  const hashed = new Set(captures(sums, / {2}(\S+)$/gm));
  // FETCH says where bytes come from, SHA256SUMS says which bytes are correct.
  // Only the second is a pin, so the manifest is exempt here for the same
  // reason scripts/verify-vendor.sh skips it.
  const exempt = new Set(["SHA256SUMS", "FETCH", "README.md", "LICENSE-three.txt", "LICENSE-three-vrm.txt", "LICENSE-jim-vrm.txt"]);
  assert.deepEqual(vendored.filter((name) => !hashed.has(name) && !exempt.has(name)), []);
  for (const license of ["LICENSE-three.txt", "LICENSE-three-vrm.txt"]) {
    assert.match(read(`web/vendor/avatar/${license}`), /MIT/, `${license} must carry its terms`);
  }

  // The model is a separate grant from the libraries, and its own metadata
  // makes credit a license term rather than a courtesy. Assert the obligation
  // is discharged everywhere it was promised, because deleting one of these
  // lines is a licensing break that nothing else would notice.
  const modelLicense = read("web/vendor/avatar/LICENSE-jim-vrm.txt");
  assert.match(modelLicense, /VirtualCast, Inc\./);
  assert.match(modelLicense, /"allowRedistribution": true|"allowRedistribution":\s*true|allowRedistribution.*true/);
  assert.match(modelLicense, /creditNotation.*required/);
  assert.match(read("web/vendor/avatar/NOTICE"), /VirtualCast, Inc\./);
  assert.match(read("README.md"), /VirtualCast, Inc\./, "README owes the model a credit");
  // Displayed in the product, not only in files a candidate never opens. It
  // sits in the lobby footer rather than under the avatar, where it crowded the
  // interview, but it has to be somewhere on screen or the license is unmet.
  assert.match(read("web/index.html"), /Seed-san by VirtualCast, Inc\./,
    "the credit is a license term and must remain visible somewhere in the UI");

  // The manifest has to name what it shipped, or the hash pins an anonymous
  // blob nobody can reproduce.
  const manifest = read("web/vendor/avatar/README.md");
  assert.match(manifest, /three.*0\.185\.1/);
  assert.match(manifest, /@pixiv\/three-vrm.*3\.5\.5/);
  assert.match(manifest, /esbuild@0\.25\.12/);

  // The bundle is served on its own URL, so the notice travels inside it and
  // not only in the sibling license files.
  const bundle = read("web/vendor/avatar/three-vrm.js");
  assert.match(bundle, /Copyright \(c\) 2019-2026 pixiv Inc\./);
  assert.match(bundle, /MIT/);

  // The hashes belong to the default gate, not only to `make check`.
  assert.match(read("scripts/test.sh"), /verify-vendor\.sh/);
  assert.match(read("Makefile"), /verify-vendor:\n\t@\.\/scripts\/verify-vendor\.sh/);
  // And the checker must find directories by glob, not by name, or a new
  // vendor directory is unpinned and silent about it.
  assert.match(read("scripts/verify-vendor.sh"), /find "\$VENDOR" -name SHA256SUMS/);
  assert.doesNotMatch(read("scripts/verify-vendor.sh"), /face-detection/);
});

test("avatar dom contract", () => {
  // The fallback is a DOM state, so it is assertable without a browser.
  assert.match(page, /id="jim-avatar"[^>]*data-avatar-state="loading"/);
  assert.match(page, /id="jim-avatar-fallback"/);
  assert.match(page, /id="jim-avatar-note"/);

  // Only "ready" hides the neutral panel. Every other state, including a state
  // nobody has invented yet, shows it.
  const css = read("web/styles.css");
  assert.match(css, /\.jim-avatar\[data-avatar-state="ready"\] \.jim-avatar-fallback \{\s*display: none;/);

  // Three.js is reachable from exactly one first-party file. If avatar.js ever
  // imports it, the node tests below stop being able to run at all.
  const importers = firstPartyScripts().filter((name) => /vendor\/avatar\/three-vrm\.js/.test(read(`web/${name}`)));
  assert.deepEqual(importers, ["avatar/vrm.js"]);

  // And it is loaded lazily, so a candidate who never starts an interview never
  // downloads 730 KB of renderer.
  assert.match(script, /import\("\.\/avatar\/vrm\.js"\)/);
  assert.doesNotMatch(script, /^import .*avatar\/vrm\.js/m);
});

test("avatar load failure", async () => {
  const mount = fakeMount();
  const avatar = createAvatar({
    mount,
    loadModel: async () => { throw new Error("no WebGL"); },
    now: () => 0,
  });
  assert.equal(mount.dataset.avatarState, "loading");
  assert.equal(await avatar.ready, false);
  assert.equal(avatar.state(), "unavailable");
  assert.equal(mount.dataset.avatarState, "unavailable");
  // Nothing to drive, and saying so beats throwing inside a frame callback.
  assert.equal(avatar.frame(0), null);
});

test("avatar load timeout", async () => {
  const mount = fakeMount();
  const avatar = createAvatar({
    mount,
    // Never settles. A model server that accepts the connection and then hangs
    // is the case a rejection test cannot reach.
    loadModel: () => new Promise(() => {}),
    timeoutMs: 20,
    now: () => 0,
  });
  assert.equal(await avatar.ready, false);
  assert.equal(mount.dataset.avatarState, "unavailable");
});

// The timeout races the load, it cannot cancel it, so the model that lost the
// race still arrives holding a WebGL context and a canvas already appended to
// the mount. Asserting only the state, which the timeout test above does, let
// that leak hide: the state is correct and the context is gone forever.
test("avatar load timeout disposes the model that arrives too late", async () => {
  const late = fakeModel();
  let settle;
  const avatar = createAvatar({
    mount: fakeMount(),
    loadModel: () => new Promise((resolve) => { settle = resolve; }),
    timeoutMs: 20,
    now: () => 0,
  });
  assert.equal(await avatar.ready, false);

  settle(late);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(late.disposed, true, "a model that lost the timeout race must be disposed, not orphaned");
  assert.equal(avatar.state(), "unavailable", "and it must not resurrect the avatar");
  assert.equal(avatar.frame(0), null);
});

test("avatar destroy during an in-flight load disposes the model that arrives after", async () => {
  const late = fakeModel();
  let settle;
  const avatar = createAvatar({
    mount: fakeMount(),
    loadModel: () => new Promise((resolve) => { settle = resolve; }),
    now: () => 0,
  });
  // loadModel is invoked on a microtask, deliberately, so that a synchronous
  // throw cannot escape createAvatar. That means it has not run yet.
  await Promise.resolve();
  avatar.destroy();
  settle(late);
  assert.equal(await avatar.ready, false);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(late.disposed, true);
  assert.equal(avatar.state(), "stopped");
});

// A loader that throws before returning a promise would otherwise escape
// withTimeout and createAvatar itself, leaving the mount on "loading" forever
// with no panel text and nothing for a caller to catch.
test("avatar survives a loader that throws synchronously", async () => {
  const mount = fakeMount();
  const avatar = createAvatar({
    mount,
    loadModel: () => { throw new Error("import failed at parse time"); },
    now: () => 0,
  });
  assert.equal(await avatar.ready, false);
  assert.equal(mount.dataset.avatarState, "unavailable");
});

test("avatar mouth opens only above the noise floor", () => {
  assert.equal(mouthFromAmplitude(0), 0);
  assert.equal(mouthFromAmplitude(MOUTH_OPEN_THRESHOLD), 0, "room tone is not speech");
  assert.equal(mouthFromAmplitude(MOUTH_FULL_AMPLITUDE), 1);
  assert.equal(mouthFromAmplitude(9), 1, "clipping does not open the jaw past its hinge");
  // Garbage in from a dead analyser must not become NaN in a bone rotation.
  assert.equal(mouthFromAmplitude(NaN), 0);
  assert.equal(mouthFromAmplitude(-1), 0);
  const middle = mouthFromAmplitude((MOUTH_OPEN_THRESHOLD + MOUTH_FULL_AMPLITUDE) / 2);
  assert.ok(middle > 0.49 && middle < 0.51, `expected a midpoint, got ${middle}`);
});

// The regression that shipped: setMouth was gated on setSpeaking(true) having
// been called, and setSpeaking is only ever called from updateAgentState, which
// returns early when it cannot find the agent participant. One stale
// "Waiting" label and Jim sat through an entire interview talking with his
// mouth shut. Audio is the signal that cannot lie about whether he is speaking,
// so the gate defaults to open and only an explicit mute closes it.
test("avatar mouth opens on audio alone, with no agent state ever reported", async () => {
  const { avatar, model } = await readyAvatar();
  // Exactly what a session with a stale agent pill does: never calls
  // setSpeaking, never calls setExpression, just pumps amplitude.
  avatar.setMouth(1);
  avatar.frame(0);
  assert.equal(model.poses.at(-1).mouth, 1, "a silent agent-state channel must not mute Jim");

  avatar.setMouth(0);
  avatar.frame(16);
  assert.equal(model.poses.at(-1).mouth, 0, "and it still tracks the amplitude down");
});

test("avatar mouth is muted only by an explicit setSpeaking(false)", async () => {
  const { avatar, model } = await readyAvatar();

  // Listening: the agent state says Jim has the floor to nobody, so the mouth
  // is held shut even if the analyser is still reporting a tail of audio.
  avatar.setSpeaking(false);
  avatar.setMouth(1);
  avatar.frame(0);
  assert.equal(model.poses.at(-1).mouth, 0, "a listening interviewer does not mouth along");

  avatar.setSpeaking(true);
  avatar.setMouth(1);
  avatar.frame(16);
  assert.equal(model.poses.at(-1).mouth, 1, "and speaking un-mutes on the next frame, not the next transition");
});

// The expression weights used to be one shared scalar across three differently
// named presets, so a state change swapped the NAME instantly while the WEIGHT
// was still the outgoing expression's: listening (relaxed 0.35) to speaking
// (happy 0.15) applied happy at 0.35, a grin at 2.2x its own target.
test("avatar state cross-fades expressions without applying one at another's weight", async () => {
  const { avatar, model } = await readyAvatar();
  avatar.frame(0);
  assert.equal(model.poses.at(-1).relaxed, undefined);
  assert.equal(model.poses.at(-1).expression.relaxed, expressionForState("listening").weight);

  avatar.setExpression("speaking");
  avatar.frame(16);
  const mid = model.poses.at(-1).expression;
  // Neither preset may exceed its own target at any point in the cross-fade.
  assert.ok(mid.happy <= expressionForState("speaking").weight + 1e-9, `happy overshot: ${mid.happy}`);
  assert.ok(mid.relaxed <= expressionForState("listening").weight + 1e-9, `relaxed overshot: ${mid.relaxed}`);
  // The outgoing preset fades rather than being cut to zero in one frame.
  assert.ok(mid.relaxed > 0, "relaxed must fade out, not vanish");

  for (let at = TRANSITION_MS; at <= TRANSITION_MS * 12; at += TRANSITION_MS) avatar.frame(at);
  const settled = model.poses.at(-1).expression;
  assert.ok(settled.relaxed < 1e-3, `relaxed must reach zero, got ${settled.relaxed}`);
  assert.ok(Math.abs(settled.happy - expressionForState("speaking").weight) < 1e-3);
  // Every preset is written every frame, so the renderer never has to remember
  // which one it set last.
  assert.deepEqual(Object.keys(settled).sort(), [...EXPRESSION_NAMES].sort());
});

// A single non-finite frame used to poison the gaze permanently, because
// NaN + (target - NaN) * step stays NaN at every subsequent frame, and that NaN
// reaches a bone rotation and makes the whole skinned mesh disappear.
test("avatar state survives a non-finite or backward clock", async () => {
  const { avatar, model } = await readyAvatar();
  avatar.setExpression("thinking");
  avatar.frame(0);
  avatar.frame(NaN);
  avatar.frame(Infinity);
  avatar.frame(-5000);
  for (let at = 16; at < 2000; at += 16) avatar.frame(at);

  const pose = model.poses.at(-1);
  for (const value of [pose.gaze.x, pose.gaze.y, pose.headTilt.x, pose.headTilt.y, pose.mouth, pose.blink, pose.breath]) {
    assert.ok(Number.isFinite(value), `a bad clock leaked a non-finite pose value: ${value}`);
  }
  assert.ok(Math.abs(pose.gaze.x - gazeForState("thinking").x) < 1e-3, "and the gaze still converges");
});

test("avatar agent state maps to a bounded pose", () => {
  for (const agentState of ["listening", "thinking", "speaking", "questioning", undefined]) {
    const gaze = gazeForState(agentState);
    assert.ok(Math.abs(gaze.x) <= GAZE_BOUND && Math.abs(gaze.y) <= GAZE_BOUND, `${agentState} gaze escaped its bound`);
    const expression = expressionForState(agentState);
    assert.ok(expression.weight >= 0 && expression.weight <= 1, `${agentState} weight escaped 0..1`);
    assert.equal(typeof expression.name, "string");
  }
  // Phase 1 reads only the three states LiveKit already publishes. An unknown
  // state falls back rather than inventing a face for it.
  assert.deepEqual(gazeForState("questioning"), gazeForState("listening"));
  assert.notDeepEqual(gazeForState("thinking"), gazeForState("listening"), "thinking looks away");
});

test("avatar state eases toward its target", async () => {
  const { avatar, model } = await readyAvatar();
  // The first frame snaps to the resting pose: easing in from a blank face at
  // startup would be an animation of the avatar arriving, which nobody asked
  // for. Only changes after that ease, so the baseline frame comes first.
  avatar.frame(0);
  assert.equal(model.poses.at(-1).gaze.x, gazeForState("listening").x);

  avatar.setExpression("thinking");
  // A quarter of the transition window gets a quarter of the way, not all of it.
  avatar.frame(TRANSITION_MS / 4);
  const partial = model.poses.at(-1).gaze.x;
  const target = gazeForState("thinking").x;
  assert.ok(partial < 0 && partial > target, `expected an eased value between 0 and ${target}, got ${partial}`);

  // Exponential smoothing approaches its target rather than landing on it, so
  // "arrived" means inside a fraction of a degree, not bit-equal. TRANSITION_MS
  // is a time constant: ~63% after one, visually finished at three.
  for (let at = TRANSITION_MS; at <= TRANSITION_MS * 12; at += TRANSITION_MS) avatar.frame(at);
  assert.ok(Math.abs(model.poses.at(-1).gaze.x - target) < 1e-3, "the gaze must arrive");

  // Frame-rate independence: the same elapsed time reaches the same place
  // whether it arrives as one long frame or eight short ones. The earlier
  // per-frame form failed this, and would have run faster on a 120 Hz display.
  const coarse = await readyAvatar();
  const fine = await readyAvatar();
  for (const each of [coarse, fine]) each.avatar.setExpression("thinking");
  coarse.avatar.frame(0);
  fine.avatar.frame(0);
  coarse.avatar.frame(TRANSITION_MS);
  for (let step = 1; step <= 8; step += 1) fine.avatar.frame((TRANSITION_MS / 8) * step);
  assert.ok(
    Math.abs(coarse.model.poses.at(-1).gaze.x - fine.model.poses.at(-1).gaze.x) < 1e-9,
    "one long frame and eight short ones must land in the same place",
  );
});

test("avatar state pushes bounded weights on every frame", async () => {
  const { avatar, model } = await readyAvatar();
  avatar.setSpeaking(true);
  // Deliberately out of range on both ends: an analyser glitch or a bad caller
  // must not reach a bone rotation.
  avatar.setGaze({ x: 99, y: -99 });
  for (let at = 0; at <= BLINK_INTERVAL_MS * 2; at += 40) {
    avatar.setMouth(at % 200 === 0 ? 5 : -5);
    avatar.frame(at);
  }
  for (const pose of model.poses) {
    assert.ok(pose.mouth >= 0 && pose.mouth <= 1, `mouth ${pose.mouth}`);
    assert.ok(pose.blink >= 0 && pose.blink <= 1, `blink ${pose.blink}`);
    for (const [name, weight] of Object.entries(pose.expression)) {
      assert.ok(weight >= 0 && weight <= 1, `${name} weight ${weight}`);
    }
    assert.ok(Math.abs(pose.gaze.x) <= GAZE_BOUND, `gaze.x ${pose.gaze.x}`);
    assert.ok(Math.abs(pose.gaze.y) <= GAZE_BOUND, `gaze.y ${pose.gaze.y}`);
    assert.ok(Math.abs(pose.headTilt.x) <= HEAD_TILT_BOUND, `headTilt.x ${pose.headTilt.x}`);
    assert.ok(Math.abs(pose.headTilt.y) <= HEAD_TILT_BOUND, `headTilt.y ${pose.headTilt.y}`);
    assert.ok(Math.abs(pose.breath) <= BREATH_AMPLITUDE, `breath ${pose.breath}`);
  }
  // The envelope is only meaningful if the frames actually blinked.
  assert.ok(model.poses.some((pose) => pose.blink > 0.5), "two blink intervals must contain a blink");
});

test("avatar interrupt zeroes the mouth without easing", async () => {
  const { avatar, model } = await readyAvatar();
  avatar.setSpeaking(true);
  avatar.setMouth(1);
  avatar.frame(0);
  avatar.frame(TRANSITION_MS * 4);
  assert.equal(model.poses.at(-1).mouth, 1);

  // Barge-in. The very next frame is shut, not a fifth of the way shut: a jaw
  // easing closed while the candidate is talking reads as Jim talking over them.
  avatar.setSpeaking(false);
  avatar.frame(TRANSITION_MS * 4 + 16);
  assert.equal(model.poses.at(-1).mouth, 0);
});

test("avatar interrupt on destroy stops the pose stream", async () => {
  const { avatar, model, mount } = await readyAvatar();
  avatar.setSpeaking(true);
  avatar.setMouth(1);
  avatar.frame(0);
  avatar.destroy();
  assert.equal(model.disposed, true);
  assert.equal(mount.dataset.avatarState, "stopped");
  assert.equal(avatar.frame(1000), null, "a destroyed avatar must not keep rendering");
  assert.equal(model.poses.length, 1);
});

test("avatar reduced-motion keeps the mouth and drops the decoration", async () => {
  const { avatar, model } = await readyAvatar({ reducedMotion: true });
  avatar.setSpeaking(true);
  avatar.setMouth(1);
  avatar.setExpression("thinking");
  // Mid-blink by the clock, and still no blink.
  avatar.frame(BLINK_DURATION_MS / 2);
  const pose = model.poses.at(-1);
  assert.equal(pose.blink, 0);
  assert.equal(pose.breath, 0);
  // Speech survives: it is a cue, not decoration. And with no easing the target
  // is reached on the first frame.
  assert.equal(pose.mouth, 1);
  assert.equal(pose.gaze.x, gazeForState("thinking").x);
  assert.equal(pose.expression.neutral, 0);

  assert.match(script, /prefers-reduced-motion: reduce/);
});

test("avatar blink and breath stay inside their envelope", () => {
  assert.equal(blinkWeight(0), 0);
  assert.equal(blinkWeight(BLINK_DURATION_MS / 2), 1);
  assert.equal(blinkWeight(BLINK_DURATION_MS), 0);
  assert.equal(blinkWeight(BLINK_INTERVAL_MS - 1), 0, "the eye is open almost all of the time");
  // Deterministic in the clock, which is the only reason the assertions above
  // can be exact; a random interval would look better and prove nothing.
  assert.equal(blinkWeight(BLINK_INTERVAL_MS + BLINK_DURATION_MS / 2), 1);
  assert.equal(breathOffset(0), 0);
  assert.ok(Math.abs(breathOffset(1234)) <= BREATH_AMPLITUDE);
});

// What the executable tests cannot see: which track the analyser is pointed at.
// Getting this wrong is not a rendering bug, it is the avatar watching the
// candidate, so it is worth a text assertion.
// Captions are a live subtitle under Jim's face, not a log; the transcript tab
// keeps every turn. Two properties, both invisible to a node test at runtime,
// so they are pinned where they are decided.
test("captions size to the turn and retire when nobody is speaking", () => {
  const css = read("web/styles.css");
  const bar = css.slice(css.indexOf(".jim-stage .captions-bar"));
  const rule = bar.slice(0, bar.indexOf("}"));
  // A fixed max-height cut sentences in half behind a scrollbar. The cap that
  // remains is viewport-relative, a backstop rather than the normal case.
  assert.doesNotMatch(rule, /max-height:\s*\d+(\.\d+)?rem;/,
    "a fixed max-height truncates the turn instead of fitting it");
  assert.match(rule, /max-height:\s*min\(40vh/);

  // Shown on every new caption, hidden after an idle stretch, and the
  // placeholder is armed too or it would sit there for the whole interview.
  assert.match(script, /const CAPTION_IDLE_HIDE_MS = \d+;/);
  const show = functionBody(script, "showCaptions");
  assert.match(show, /nodes\.captionsBar\.hidden = false;/);
  assert.match(show, /clearTimeout\(captionIdleTimer\)/, "a new turn must reset the countdown, not stack timers");
  assert.match(show, /setTimeout\([\s\S]*?nodes\.captionsBar\.hidden = true;[\s\S]*?CAPTION_IDLE_HIDE_MS\)/);
  assert.match(script, /nodes\.captionsText\.textContent = `\[\$\{speaker[^`]*`;\n\s*showCaptions\(\);/,
    "every caption update must un-hide the bar");
});

test("avatar analyser reads Jim and never the candidate", () => {
  assert.match(script, /createMediaStreamSource\(new MediaStream\(\[track\.mediaStreamTrack\]\)\)/);
  // createMediaElementSource returns silence for a MediaStream-backed element,
  // so the mouth would never open and nothing would say why. Matching the call
  // rather than the name, because the comment that warns about it names it too.
  assert.doesNotMatch(script, /createMediaElementSource\s*\(/);
  // The analyser is built where Jim's track arrives, and nowhere else.
  const sites = [...script.matchAll(/attachAvatarAnalyser\(/g)].length;
  assert.equal(sites, 2, "one definition, one call site in playRemoteAudio");
  assert.match(functionBody(script, "playRemoteAudio"), /attachAvatarAnalyser\(track, participant\)/);
  // Jim only. playRemoteAudio fires for every remote audio track, so without
  // this the analyser followed whichever arrived first, which in a room with a
  // stray hosted agent or a second participant is not the interviewer.
  assert.match(functionBody(script, "attachAvatarAnalyser"), /if \(!isAgent\(participant\)\) return;/);
  // And the teardown is gated on the analyser's own track, or a second
  // participant leaving killed lip sync for the rest of the session.
  assert.match(script, /if \(track !== jimAnalyserTrack\) return;/);
  // Never the microphone, checked across the whole file rather than inside one
  // function slice: wiring the candidate's stream in from anywhere else would
  // have passed a slice-scoped assertion while the avatar watched the candidate.
  // An allowlist, not a count. The preflight legitimately builds a source from
  // the candidate's own microphone to drive the level meter, so the property
  // worth pinning is that the set does not grow: exactly two call sites, and
  // the avatar's is the remote track.
  const sources = captures(script, /createMediaStreamSource\(([^)]*)\)/g);
  assert.deepEqual(sources, ["stream", "new MediaStream([track.mediaStreamTrack]"],
    "a new createMediaStreamSource call site appeared; the avatar must only ever read Jim");
  assert.doesNotMatch(script, /createMediaStreamSource\([^)]*localUserStream/);
  assert.ok(ANALYSER_WINDOW > 1, "a single frame of amplitude flickers the jaw");

  // Not finding an agent participant is the exact condition that pinned Jim's
  // jaw shut for whole interviews. Muting on it re-creates that bug in the one
  // case the audio-driven mouth exists to survive, and a review caught it after
  // it had already been written back in once.
  const branch = functionBody(script, "updateAgentState").split("state.sawAgent = true")[0];
  assert.doesNotMatch(branch, /setSpeaking\(/,
    "the no-agent branch must not mute the mouth; silence closes it on its own");

  // The source node is released, not just dropped. LiveKit re-subscribes Jim on
  // every reconnect, so a source left connected accumulates one per reconnect.
  assert.match(script, /jimAnalyserSource\?\.disconnect\(\)/);
  for (const caller of ["dropRemoteAudio", "stopAvatar"]) {
    assert.match(functionBody(script, caller), /releaseAvatarAnalyser\(\)/, `${caller} must release the analyser`);
  }
  // A context built outside a user gesture starts suspended and reads silence.
  assert.match(script, /jimAnalyserContext\.state === "suspended"/);
});
