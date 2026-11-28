import { test } from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import {
  createAudioPlayback,
  playbackBlocked,
} from "../../web/audio-playback.js";
import { loadInterview } from "./source.js";
import { createJimVolume } from "../../web/audio-output.js";

const blocked = () => new DOMException("Gesture required", "NotAllowedError");
const tick = () => new Promise((resolve) => setImmediate(resolve));
const denied = () => ({ play: () => Promise.reject(blocked()) });
const playing = () => ({ play: async () => {} });
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function fixture({ reapply = () => {} } = {}) {
  const notice = { hidden: true };
  const document = { activeElement: null };
  const button = new EventTarget();
  button.ownerDocument = document;
  button.attributes = {};
  button.setAttribute = (name, value) => (button.attributes[name] = value);
  const focusTarget = { focus: () => (document.activeElement = focusTarget) };
  const announcement = { textContent: "" };
  const room = new EventEmitter();
  room.canPlaybackAudio = true;
  let starts = 0;
  room.startAudio = async () => starts++;
  const playback = createAudioPlayback({
    notice,
    button,
    focusTarget,
    announcement,
    reapply,
  });
  playback.setRoom(room, "playback");
  return {
    playback,
    room,
    notice,
    button,
    document,
    focusTarget,
    announcement,
    starts: () => starts,
    click: () => button.dispatchEvent(new Event("click")),
    status: (allowed) => {
      room.canPlaybackAudio = allowed;
      room.emit("playback");
    },
  };
}

test("only observed denial blocks playback; unrelated errors preserve state", () => {
  const status = (allowed) => ({ type: "status", allowed });
  const rejected = (name) => ({ type: "rejected", name });
  assert.equal(playbackBlocked(false, status(false)), true);
  assert.equal(playbackBlocked(false, rejected("NotAllowedError")), true);
  assert.equal(playbackBlocked(false, rejected("AbortError")), false);
  assert.equal(playbackBlocked(true, rejected("AbortError")), true);
  assert.equal(playbackBlocked(true, { type: "status" }), true);
  assert.equal(playbackBlocked(true, { type: "played" }), false);
  assert.equal(playbackBlocked(true, status(true)), false);
});

test("play rejection shows recovery and a successful play clears it", async () => {
  const f = fixture();
  await f.playback.play(denied());
  assert.equal(f.notice.hidden, false);
  await f.playback.play(playing());
  assert.equal(f.notice.hidden, true);
});

test("room status is ignored before Jim has an element", () => {
  const f = fixture();
  f.status(false);
  assert.equal(f.notice.hidden, true);
});

test("status updates show recovery and return focus before hiding it", async () => {
  const f = fixture();
  await f.playback.play(playing());
  f.status(false);
  assert.equal(f.notice.hidden, false);
  f.document.activeElement = f.button;
  f.status(true);
  assert.equal(f.notice.hidden, true);
  assert.equal(f.document.activeElement, f.focusTarget);
});

test("resume starts both calls synchronously and deduplicates pending clicks", async () => {
  const f = fixture();
  const held = deferred();
  let plays = 0;
  const element = {
    play: () => {
      plays++;
      return plays === 1 ? Promise.reject(blocked()) : held.promise;
    },
  };
  await f.playback.play(element);
  f.click();
  f.click();
  assert.equal(f.starts(), 1);
  assert.equal(plays, 2);
  assert.equal(f.button.attributes["aria-busy"], "true");
  held.resolve();
  await tick();
  assert.equal(f.notice.hidden, true);
  assert.equal(f.button.attributes["aria-busy"], "false");
});

test("failed resume stays visible and can be retried", async () => {
  const f = fixture();
  await f.playback.play(denied());
  f.room.startAudio = () => {
    throw blocked();
  };
  f.click();
  await tick();
  assert.equal(f.notice.hidden, false);
  assert.equal(f.button.attributes["aria-busy"], "false");
  f.click();
  assert.equal(f.button.attributes["aria-busy"], "true");
  await tick();
});

test("old play promises cannot block a replacement source", async () => {
  const f = fixture();
  const old = deferred();
  const first = f.playback.play({ play: () => old.promise });
  await f.playback.play(playing());
  old.reject(blocked());
  await first;
  assert.equal(f.notice.hidden, true);
});

test("old successful play cannot clear a replacement denial", async () => {
  const f = fixture();
  const old = deferred();
  const first = f.playback.play({ play: () => old.promise });
  await f.playback.play(denied());
  old.resolve();
  await first;
  assert.equal(f.notice.hidden, false);
});

test("a play success clears a room block without needing a matching status event", async () => {
  const f = fixture();
  const held = deferred();
  const first = f.playback.play({ play: () => held.promise });
  f.status(false);
  held.resolve();
  await first;
  assert.equal(f.notice.hidden, true);
});

test("teardown removes listeners and ignores a pending resume", async () => {
  const f = fixture();
  const held = deferred();
  let calls = 0;
  await f.playback.play({
    play: () => (++calls === 1 ? Promise.reject(blocked()) : held.promise),
  });
  f.click();
  f.playback.setRoom(null);
  assert.equal(f.room.listenerCount("playback"), 0);
  held.reject(blocked());
  await tick();
  f.status(false);
  assert.equal(f.notice.hidden, true);
});

test("dropping another element does not clear the current source", async () => {
  const f = fixture();
  const current = denied();
  await f.playback.play(current);
  f.playback.forget({});
  assert.equal(f.notice.hidden, false);
  f.playback.forget(current);
  assert.equal(f.notice.hidden, true);
});

test("replacing a room detaches its status listener", async () => {
  const f = fixture();
  await f.playback.play(denied());
  const next = new EventEmitter();
  next.canPlaybackAudio = true;
  f.playback.setRoom(next, "playback");
  await f.playback.play(playing());
  f.status(false);
  assert.equal(f.notice.hidden, true);
  assert.equal(f.room.listenerCount("playback"), 0);
  assert.equal(next.listenerCount("playback"), 1);
});

test("releasing a room that was already replaced keeps the current one", async () => {
  const f = fixture();
  const earlier = new EventEmitter();
  earlier.canPlaybackAudio = true;
  f.playback.release(earlier);
  await f.playback.play(denied());
  f.playback.release(earlier);
  assert.equal(f.notice.hidden, false);
  assert.equal(f.room.listenerCount("playback"), 1);
  f.playback.release(f.room);
  assert.equal(f.notice.hidden, true);
  assert.equal(f.room.listenerCount("playback"), 0);
});

test("late track playback after teardown does not play or show recovery", async () => {
  const f = fixture();
  f.playback.setRoom(null);
  let plays = 0;
  await f.playback.play({
    play: () => {
      plays++;
      return Promise.reject(blocked());
    },
  });
  assert.equal(plays, 0);
  assert.equal(f.notice.hidden, true);
});

test("a pending recovery cannot overwrite a replacement element's denial", async () => {
  const f = fixture();
  const held = deferred();
  let calls = 0;
  await f.playback.play({
    play: () => (++calls === 1 ? Promise.reject(blocked()) : held.promise),
  });
  f.click();
  await f.playback.play(denied());
  held.resolve();
  await tick();
  assert.equal(f.notice.hidden, false);
  assert.equal(f.button.attributes["aria-busy"], "false");
});

test("reattaching blocked audio never hides the notice or steals button focus", async () => {
  const f = fixture();
  await f.playback.play(denied());
  f.document.activeElement = f.button;
  const visibility = [];
  Object.defineProperty(f.notice, "hidden", {
    get: () => visibility[visibility.length - 1] ?? false,
    set: (hidden) => {
      visibility.push(hidden);
    },
  });
  await f.playback.play(denied());
  assert.ok(visibility.length > 0);
  assert.equal(visibility.includes(true), false);
  assert.equal(f.document.activeElement, f.button);
});

test("the current play rejection wins over a prior allowed room event", async () => {
  const f = fixture();
  const held = deferred();
  const playing = f.playback.play({ play: () => held.promise });
  f.status(true);
  held.reject(blocked());
  await playing;
  assert.equal(f.notice.hidden, false);
});

test("actual element playback clears recovery even when room recovery fails", async () => {
  const f = fixture();
  let calls = 0;
  await f.playback.play({
    play: () => (++calls === 1 ? Promise.reject(blocked()) : Promise.resolve()),
  });
  f.room.startAudio = () => Promise.reject(blocked());
  f.click();
  await tick();
  assert.equal(f.notice.hidden, true);
  assert.equal(f.announcement.textContent, "Audio restored.");
});

test("reset clears recovery guard for a new room without announcing restoration", async () => {
  const f = fixture();
  const held = deferred();
  await f.playback.play(denied());
  f.room.startAudio = () => held.promise;
  f.click();
  f.playback.setRoom(null);
  assert.equal(f.button.attributes["aria-busy"], "false");
  assert.equal(f.announcement.textContent, "");
  const next = new EventEmitter();
  next.canPlaybackAudio = false;
  let starts = 0;
  next.startAudio = async () => starts++;
  f.playback.setRoom(next, "playback");
  await f.playback.play(denied());
  f.click();
  assert.equal(starts, 1);
  held.resolve();
  await tick();
});

/// One function from interview.js, with everything it reads beyond `own`
/// stubbed.
const interviewFunction = (name, own) =>
  loadInterview([name], own, { stubMissing: true })[name];

/// LiveKit's own detach pauses every element it lets go of, which is what a
/// late unsubscribe must not reach.
function audioTrack() {
  return {
    kind: "audio",
    elements: [],
    attach(element) {
      this.elements.push(element);
    },
    detach(only) {
      const gone = only
        ? this.elements.filter((e) => e === only)
        : this.elements;
      this.elements = this.elements.filter((e) => !gone.includes(e));
      for (const element of gone) element.pause();
      return gone;
    },
  };
}

test("releasing a routing hold detects blocked audible play and offers recovery", async () => {
  const route = deferred();
  let allowed = false;
  const element = {
    volume: 1,
    muted: false,
    isConnected: false,
    play() {
      return this.muted || allowed
        ? Promise.resolve()
        : Promise.reject(blocked());
    },
  };
  let volume;
  const f = fixture({ reapply: () => volume.apply() });
  const own = {
    jimAudio: null,
    jimTrack: null,
    audioPlayback: f.playback,
    isCurrentAgent: () => true,
    applyAudioOutput: async () => {},
    audioOutputSettled: () => route.promise,
    setTimeout: () => 1,
    clearTimeout() {},
    setBanner() {},
    document: {
      createElement: () => element,
      body: { appendChild: (audio) => (audio.isConnected = true) },
    },
  };
  volume = own.jimVolume = createJimVolume({
    element: () => own.jimAudio,
    storage: null,
  });
  interviewFunction("playRemoteAudio", own)({ kind: "audio", attach() {} }, {});
  await tick();
  assert.equal(element.muted, true);
  assert.equal(f.notice.hidden, true);
  route.resolve();
  await tick();
  assert.equal(element.muted, false);
  assert.equal(f.notice.hidden, false);
  allowed = true;
  f.click();
  await tick();
  assert.equal(f.notice.hidden, true);
});

test("a native pause after successful play rechecks permission and drops old listeners", async () => {
  const f = fixture();
  let allowed = true;
  let attempts = 0;
  const audio = Object.assign(new EventTarget(), {
    paused: false,
    ended: false,
    play() {
      attempts += 1;
      return allowed ? Promise.resolve() : Promise.reject(blocked());
    },
  });
  await f.playback.play(audio);
  allowed = false;
  audio.paused = true;
  audio.dispatchEvent(new Event("pause"));
  await tick();
  assert.equal(attempts, 2);
  assert.equal(f.notice.hidden, false);
  f.playback.forget(audio);
  audio.dispatchEvent(new Event("pause"));
  await tick();
  assert.equal(attempts, 2);
  assert.equal(f.notice.hidden, true);
});

test("fresh Jim audio stays muted until the selected output route settles", async () => {
  const route = deferred();
  const observed = [];
  const element = { muted: false, volume: 1, isConnected: false };
  let expire;
  const messages = [];
  const own = {
    jimAudio: null,
    jimTrack: null,
    isCurrentAgent: () => true,
    audioOutputId: () => "headphones",
    setTimeout(callback) {
      expire = callback;
      return 1;
    },
    clearTimeout() {},
    setBanner(source, message) {
      messages.push([source, message]);
    },
    applyAudioOutput(id) {
      assert.equal(id, "headphones");
      return Promise.resolve();
    },
    audioOutputSettled() {
      return route.promise;
    },
    audioPlayback: {
      play(audio) {
        observed.push(["play", audio.muted]);
        return Promise.resolve();
      },
    },
    document: {
      createElement: () => element,
      body: { appendChild: (audio) => (audio.isConnected = true) },
    },
  };
  own.jimVolume = createJimVolume({
    element: () => own.jimAudio,
    storage: null,
  });
  const track = {
    kind: "audio",
    attach(audio) {
      audio.muted = false;
      observed.push(["attach", audio.muted]);
    },
  };
  const play = interviewFunction("playRemoteAudio", own);
  play(track, {});
  assert.deepEqual(observed, [
    ["attach", false],
    ["play", true],
  ]);
  await tick();
  assert.equal(element.muted, true);
  expire();
  assert.match(messages.at(-1)[1], /reload/);
  route.resolve();
  await tick();
  assert.equal(element.muted, false);
  assert.deepEqual(messages.at(-1), ["output", ""]);
  play(track, {});
  assert.equal(own.jimAudio, element);
  assert.deepEqual(observed.slice(-2), [
    ["attach", false],
    ["play", false],
  ]);
});

test("a late unsubscribe for Jim's old track leaves the new track playing and recoverable", async () => {
  const f = fixture();
  const own = {
    jimAudio: null,
    jimTrack: null,
    audioPlayback: f.playback,
    applyAudioOutput: async () => {},
    audioOutputSettled: async () => {},
    isCurrentAgent: () => true,
    isAvatarAnalyserTrack: () => false,
    document: {
      createElement: () => ({
        isConnected: false,
        paused: true,
        play() {
          this.paused = false;
          return Promise.resolve();
        },
        pause() {
          this.paused = true;
        },
        remove() {
          this.isConnected = false;
        },
      }),
      body: { appendChild: (element) => (element.isConnected = true) },
    },
  };
  own.jimVolume = createJimVolume({
    element: () => own.jimAudio,
    storage: null,
  });
  const play = interviewFunction("playRemoteAudio", own);
  const drop = interviewFunction("dropRemoteAudio", own);
  const old = audioTrack();
  const replacement = audioTrack();
  play(old, {});
  await tick();
  const element = own.jimAudio;
  assert.equal(element.muted, false);
  // Reconnect order LiveKit is allowed to use: the new subscribe first.
  play(replacement, {});
  await tick();
  f.status(false);
  drop(old);
  assert.equal(own.jimAudio, element);
  assert.equal(element.isConnected, true);
  assert.equal(element.muted, false);
  assert.equal(element.paused, false);
  assert.equal(f.notice.hidden, false);
  drop(replacement);
  assert.equal(own.jimAudio, null);
  assert.equal(element.isConnected, false);
  assert.equal(f.notice.hidden, true);
});

test("report recovery keeps playback tracked; only the rendered report ends it", async () => {
  const f = fixture();
  const own = { audioPlayback: f.playback };
  await f.playback.play(denied());
  interviewFunction("stopLocalMedia", own)();
  assert.equal(f.notice.hidden, false);
  assert.equal(f.room.listenerCount("playback"), 1);
  interviewFunction("renderReport", own)();
  assert.equal(f.notice.hidden, true);
  assert.equal(f.room.listenerCount("playback"), 0);
});

test("playback recovery restores session mute after LiveKit unmutes its elements", async () => {
  const { createJimVolume } = await import("../../web/audio-output.js");
  let refused = true;
  const element = {
    volume: 1,
    muted: false,
    play: async () => {
      if (refused) throw blocked();
    },
  };
  const volume = createJimVolume({ element: () => element, storage: null });
  volume.setVolume(25);
  volume.toggleMute();
  const f = fixture({ reapply: () => volume.apply() });
  await f.playback.play(element);
  f.room.startAudio = async () => {
    element.muted = false;
  };
  refused = false;
  f.click();
  await tick();
  assert.equal(f.notice.hidden, true);
  assert.equal(element.muted, true);
  assert.equal(element.volume, 0.25);
  element.muted = false;
  f.status(true);
  assert.equal(element.muted, true);
});

test("a late old-room recovery reapplies preferences to the currently attached element", async () => {
  const { createJimVolume } = await import("../../web/audio-output.js");
  let current = { ...denied(), volume: 1, muted: false };
  const volume = createJimVolume({ element: () => current, storage: null });
  volume.toggleMute();
  const f = fixture({ reapply: () => volume.apply() });
  await f.playback.play(current);
  const held = deferred();
  f.room.startAudio = () =>
    held.promise.then(() => {
      current.muted = false;
    });
  f.click();
  // The interview reconnects: a new room and a new element arrive while the
  // old room's recovery is still pending.
  const next = new EventEmitter();
  next.canPlaybackAudio = true;
  next.startAudio = async () => {};
  f.playback.setRoom(next, "playback");
  current = { ...playing(), volume: 1, muted: false };
  await f.playback.play(current);
  held.resolve();
  await tick();
  assert.equal(current.muted, true);
});

test("synchronous startAudio unmute is corrected before a stalled resume can play", async () => {
  let muted = true;
  const playedMuted = [];
  const element = {
    play: async () => {
      playedMuted.push(muted);
      throw blocked();
    },
  };
  const f = fixture({
    reapply: () => {
      muted = true;
    },
  });
  await f.playback.play(element);
  const held = deferred();
  f.room.startAudio = () => {
    muted = false;
    return held.promise;
  };
  playedMuted.length = 0;
  f.click();
  assert.equal(muted, true);
  assert.deepEqual(playedMuted, [true]);
  held.resolve();
  await tick();
});
