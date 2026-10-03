import { test } from "node:test";
import assert from "node:assert/strict";

import { attachSelfView } from "../../web/self-view.js";
import { functionBody, interviewSource } from "./source.js";

globalThis.MediaStream = class {
  constructor(tracks) {
    this.tracks = tracks;
  }
};

class FakeTrack {
  constructor({ readyState = "live", muted = false } = {}) {
    this.kind = "video";
    this.readyState = readyState;
    this.muted = muted;
    this.listeners = new Map();
  }
  addEventListener(type, listener) {
    this.listeners.set(type, listener);
  }
  removeEventListener(type, listener) {
    if (this.listeners.get(type) === listener) this.listeners.delete(type);
  }
  fire(type) {
    this.listeners.get(type)?.();
  }
}

const streamOf = (...tracks) => ({ getVideoTracks: () => tracks });

// Every element starts visible and holding a stream. A case asserting "hidden"
// or "cleared" against an element that already was would pass with the module
// doing nothing at all.
const staleElement = () => ({
  hidden: false,
  srcObject: { stale: true },
  play: async () => {},
});

test("a live camera is shown", () => {
  const video = { hidden: true, srcObject: null, play: async () => {} };
  const track = new FakeTrack();
  attachSelfView(video, streamOf(track));
  assert.equal(video.hidden, false);
  assert.deepEqual(video.srcObject.tracks, [track]);
  assert.equal(
    video.muted,
    true,
    "the self view must never play the candidate back to themselves",
  );
});

// A camera the candidate skipped, or one handed to Meet, is simply absent
// from the stream by the time this runs.
test("no camera shows nothing", () => {
  for (const stream of [streamOf(), null]) {
    const video = staleElement();
    attachSelfView(video, stream);
    assert.equal(video.hidden, true);
    assert.equal(video.srcObject, null);
  }
});

test("a camera that ended before the room shows nothing", () => {
  const video = staleElement();
  attachSelfView(video, streamOf(new FakeTrack({ readyState: "ended" })));
  assert.equal(video.hidden, true);
  assert.equal(video.srcObject, null);
});

// Another application taking the camera leaves the track live and muted, and
// the element would keep painting the last frame as if it were the candidate.
test("a camera another application takes is hidden until it comes back", () => {
  const video = staleElement();
  const track = new FakeTrack();
  attachSelfView(video, streamOf(track));
  track.muted = true;
  track.fire("mute");
  assert.equal(video.hidden, true);
  track.muted = false;
  track.fire("unmute");
  assert.equal(video.hidden, false);
});

test("a camera that is already muted when attached starts hidden", () => {
  const video = staleElement();
  attachSelfView(video, streamOf(new FakeTrack({ muted: true })));
  assert.equal(video.hidden, true);
});

test("a camera that ends mid-interview is taken down", () => {
  const video = staleElement();
  const track = new FakeTrack();
  attachSelfView(video, streamOf(track));
  track.readyState = "ended";
  track.fire("ended");
  assert.equal(video.hidden, true);
  assert.equal(video.srcObject, null);
});

// Stopping a track fires no `ended`, so the teardown is the only thing that
// clears the element when the interview ends.
test("detaching clears the element and stops listening", () => {
  const video = staleElement();
  const track = new FakeTrack();
  const detach = attachSelfView(video, streamOf(track));
  assert.equal(track.listeners.size, 3);
  detach();
  assert.equal(video.hidden, true);
  assert.equal(video.srcObject, null);
  assert.equal(track.listeners.size, 0);
});

// Nothing here can execute `web/interview.js`, so the wiring is pinned as text.
// A tripwire, not a proof: it catches the two orderings that matter moving.
test("the interview shows the camera after a Meet release and takes it down on teardown", () => {
  const script = interviewSource();
  const release = script.indexOf(
    "releaseCameraToPresenter(preflight.userStream)",
  );
  const attach = script.indexOf(
    "attachSelfView(nodes.selfView, preflight.userStream)",
  );
  const joinsRoom = script.indexOf("await connect(preflight");
  assert.notEqual(attach, -1, "init must attach the self view");
  assert.ok(
    release < attach,
    "attached before the release, the self view would keep a camera Meet was handed",
  );
  assert.ok(
    attach < joinsRoom,
    "attached before the room, so the candidate sees themselves while it connects",
  );

  const teardown = functionBody(script, "stopLocalMedia");
  const detach = teardown.indexOf("detachSelfView();");
  const stop = teardown.indexOf("stopPreflight(");
  assert.notEqual(detach, -1, "stopLocalMedia must take the self view down");
  assert.ok(
    detach < stop,
    "down before the tracks stop, which fires no ended to do it",
  );
});
