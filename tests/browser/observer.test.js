import { test } from "node:test";
import assert from "node:assert/strict";
import { read } from "./source.js";

const page = read("web/observer.html");
const script = read("web/observer.js");
const observerSource = `${page}\n${script}`;

test("observer page is a deliberate subscribe-only join", () => {
  assert.match(page, /id="join-observer"/);
  assert.match(page, /Observers cannot speak or use a camera or microphone/);
  assert.match(page, /candidate has been told you are joining/);
  assert.match(script, /"\/api\/observer-token"/);
  assert.match(script, /addEventListener\("click"/);
  assert.match(script, /room\.connect\(connection\.serverUrl, connection\.token\)/);
  assert.match(script, /TrackSubscribed/);
  assert.match(script, /track\.attach\(\)/);
});

test("observer page never captures, publishes, or enumerates devices", () => {
  assert.doesNotMatch(observerSource, /getUserMedia|createLocalTracks|publishTrack|publishData|enumerateDevices/);
});
