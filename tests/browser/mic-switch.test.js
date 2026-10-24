// Run with: node --test tests/browser/mic-switch.test.js
//
// Switching the microphone is an ordering problem: open the new input, hand it
// to the published track, then retire the old one, and back out cleanly at
// every step that can fail. These drive the real switch against fakes for the
// stream and the LiveKit track, so each step is executed rather than read.

import { test } from "node:test";
import assert from "node:assert/strict";
import { FakeStream, FakeTrack } from "./fake-media.js";
import { loadInterview } from "./source.js";

import {
  createMicrophoneSwitch,
  holdMute,
  inputOptions,
  liftMute,
  switchDetail,
} from "../../web/mic-switch.js";

/// A microphone that reports the device it opened, which is what the switch
/// compares a pick against and what the select marks as in use.
class FakeMic extends FakeTrack {
  constructor(deviceId, label = deviceId) {
    super("audio");
    this.deviceId = deviceId;
    this.label = label;
  }
  getSettings() {
    return { deviceId: this.deviceId };
  }
}

/// Stands in for LiveKit's LocalAudioTrack. Like the real one, it re-enables
/// the track it is handed from its own mute flag, which the page never sets.
class FakePublished {
  constructor(track) {
    this.sending = track;
    this.calls = [];
    this.refuse = false;
    // Refuses every call after this many, so a test can fail only a later one.
    this.refuseAfter = Infinity;
    // Set by a test to hold the replace open while it changes the session.
    this.pending = null;
    // Whether each track was enabled at the moment the sender took it.
    this.sentEnabled = [];
    // LiveKit's own mute flag, which it raises when a user-provided track ends.
    this.isMuted = false;
    this.liveAfterReplace = [];
    this.unmutes = 0;
    this.liveAfterUnmute = [];
    this.listeners = {};
  }
  async replaceTrack(track, options) {
    this.calls.push(options);
    this.sentEnabled.push(track.enabled);
    // LiveKit's setMediaStreamTrack returns at once for the track it already
    // holds, so a hand-back to an input never replaced cannot fail.
    if (track === this.sending) return;
    if (this.pending) await this.pending;
    if (this.refuse || this.calls.length > this.refuseAfter)
      throw new Error("sender closed");
    // What LiveKit does when a user-provided track it is listening to ends.
    if (this.muteOnEnded && track.readyState === "ended") this.isMuted = true;
    // LiveKit hands the track to the sender and then resumes upstream, which
    // can still reject with the new track already being sent.
    // One-shot, so only the replace under test rejects.
    if (this.rejectAfterSend) {
      this.rejectAfterSend = false;
      this.sending = track;
      throw new Error("resume upstream failed");
    }
    track.enabled = !this.isMuted;
    // What the sender was given to send once LiveKit had set it up, which is
    // the window a muted candidate must not be live in.
    this.liveAfterReplace.push(track.enabled);
    this.sending = track;
  }
  async mute() {
    this.isMuted = true;
    this.sending.enabled = false;
  }
  // LiveKit's setTrackMuted: enable the track, then announce it in the same
  // synchronous step.
  async unmute() {
    if (this.unmutePending) await this.unmutePending;
    if (this.refuseUnmute) throw new Error("unmute refused");
    this.unmutes += 1;
    this.isMuted = false;
    this.sending.enabled = true;
    for (const listener of [...(this.listeners.unmuted || [])]) listener(this);
    // What is left enabled once LiveKit's own step is over.
    this.liveAfterUnmute.push(this.sending.enabled);
  }
  get mediaStreamTrack() {
    return this.sending;
  }
  on(name, listener) {
    (this.listeners[name] ||= []).push(listener);
  }
  off(name, listener) {
    this.listeners[name] = (this.listeners[name] || []).filter(
      (each) => each !== listener,
    );
  }
}

function setup({ open, live = true, muted = false, published = true } = {}) {
  const old = new FakeMic("laptop");
  const stream = new FakeStream([old]);
  const sender = published ? new FakePublished(old) : null;
  const state = {
    live,
    muted,
    opened: [],
    switched: [],
    failed: [],
    outcomes: [],
    dropped: [],
  };
  const microphone = createMicrophoneSwitch({
    open: async (deviceId) => {
      state.opened.push(deviceId);
      return open ? open(deviceId) : new FakeMic(deviceId);
    },
    streamOf: () => stream,
    publishedTrack: () => sender,
    isLive: () => state.live,
    micEnabled: () => !state.muted,
    onSwitched: (track, reason) => state.switched.push({ track, reason }),
    onFailed: (reason, error, outcome, droppedLive) => {
      state.failed.push(reason);
      state.outcomes.push(outcome);
      state.dropped.push(droppedLive);
    },
  });
  return { microphone, old, stream, sender, state };
}

const audioIn = (stream) =>
  stream.getTracks().filter((t) => t.kind === "audio");

test("a chosen input replaces the sent track and retires the old one", async () => {
  const { microphone, old, stream, sender, state } = setup();

  assert.equal(await microphone.switchTo("headset"), "switched");

  const [now] = audioIn(stream);
  assert.equal(now.deviceId, "headset");
  assert.equal(audioIn(stream).length, 1, "the stream holds exactly one input");
  assert.equal(sender.sending, now, "LiveKit sends the input the stream holds");
  assert.deepEqual(sender.calls, [{ userProvidedTrack: true }]);
  assert.ok(old.stopped, "the previous device stays open");
  const switched = state.switched.map((s) => [s.reason, s.track.deviceId]);
  assert.deepEqual(switched, [["chosen", "headset"]]);
});

// The Mic button mutes by disabling the track. LiveKit's replace re-enables
// whatever it is given, so a muted candidate who switches would be live again.
test("a muted microphone stays muted across a switch", async () => {
  const { microphone, stream, sender } = setup({ muted: true });

  await microphone.switchTo("headset");

  assert.equal(audioIn(stream)[0].enabled, false);
  assert.deepEqual(
    sender.sentEnabled,
    [false],
    "the sender took the new input while it was still enabled",
  );
  // LiveKit enables the new track from its own flag inside the replace, so the
  // flag has to read muted by then, or the input is live until the page
  // disables it again.
  assert.deepEqual(sender.liveAfterReplace, [false]);
  // And down again afterwards, without the input going live as it comes down:
  // a raised flag is a server-side mute the agent hears as no frames at all.
  assert.equal(sender.isMuted, false);
  assert.deepEqual(sender.liveAfterUnmute, [false]);
});

test("an input that will not open leaves the current one in use", async () => {
  const { microphone, old, stream, sender, state } = setup({
    open: () => {
      throw new Error("NotReadableError");
    },
  });

  assert.equal(await microphone.switchTo("headset"), "refused");

  assert.deepEqual(audioIn(stream), [old]);
  assert.equal(old.stopped, false);
  assert.equal(sender.sending, old);
  assert.deepEqual(state.failed, ["chosen"]);
  assert.deepEqual(state.switched, []);
});

test("a refused replace stops the new device and keeps the old", async () => {
  const opened = [];
  const { microphone, old, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  sender.refuse = true;

  assert.equal(await microphone.switchTo("headset"), "refused");

  assert.ok(opened[0].stopped, "the device opened for the switch was left on");
  assert.deepEqual(audioIn(stream), [old]);
  assert.equal(old.stopped, false);
  assert.deepEqual(state.failed, ["chosen"]);
});

// A permission prompt can stay up past the end of the interview.
test("an input granted after the interview ended is stopped, not swapped in", async () => {
  let grant = null;
  const pending = new FakeMic("headset");
  const { microphone, old, stream, state } = setup({
    open: () =>
      new Promise((resolve) => {
        grant = () => resolve(pending);
      }),
  });

  const result = microphone.switchTo("headset");
  await new Promise((resolve) => setTimeout(resolve, 0));
  state.live = false;
  grant();

  assert.equal(await result, "idle");
  assert.ok(pending.stopped);
  assert.deepEqual(audioIn(stream), [old]);
  assert.deepEqual(state.switched, []);
});

test("nothing is opened while there is no published track to hand it to", async () => {
  const { microphone, state } = setup({ published: false });

  assert.equal(await microphone.switchTo("headset"), "idle");
  assert.deepEqual(state.opened, []);
});

test("picking the input already in use opens nothing", async () => {
  const { microphone, state } = setup();

  assert.equal(await microphone.switchTo("laptop"), "unchanged");
  assert.deepEqual(state.opened, []);
});

// Two picks while the first prompt is still up. Unserialized, the first could
// resolve last and leave the stream and LiveKit holding different inputs.
test("rapid picks resolve in the order they were made", async () => {
  const grants = [];
  const tracks = [];
  const { microphone, stream, sender } = setup({
    open: (deviceId) =>
      new Promise((resolve) => {
        const track = new FakeMic(deviceId);
        tracks.push(track);
        grants.push(() => resolve(track));
      }),
  });

  const first = microphone.switchTo("headset");
  const second = microphone.switchTo("webcam");
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(grants.length, 1, "the second pick waits for the first");
  grants[0]();
  assert.equal(await first, "switched");
  await new Promise((resolve) => setTimeout(resolve, 0));
  grants[1]();
  assert.equal(await second, "switched");

  assert.equal(audioIn(stream)[0].deviceId, "webcam");
  assert.equal(sender.sending, audioIn(stream)[0]);
  assert.ok(tracks[0].stopped, "the input switched away from stays open");
});

test("an input that ended moves the interview to the default", async () => {
  const { microphone, old, stream, state } = setup();
  old.readyState = "ended";

  assert.equal(await microphone.switchTo("", "lost", old), "switched");

  assert.deepEqual(state.opened, [""], "the default is asked for by no id");
  assert.equal(audioIn(stream).length, 1);
  assert.notEqual(audioIn(stream)[0], old);
  assert.equal(state.switched[0].reason, "lost");
});

// The pick replaced the track that later ended, so the loss is about an input
// the interview no longer uses.
test("a loss queued behind a pick leaves the picked input in use", async () => {
  let grant = null;
  const { microphone, old, stream, state } = setup({
    // Only the pick waits on its prompt, so a loss that wrongly runs resolves
    // and fails the assertions rather than hanging the test.
    open: (deviceId) =>
      grant
        ? new FakeMic(deviceId)
        : new Promise((resolve) => {
            grant = () => resolve(new FakeMic(deviceId));
          }),
  });

  const picked = microphone.switchTo("headset");
  await new Promise((resolve) => setTimeout(resolve, 0));
  old.readyState = "ended";
  const lost = microphone.switchTo("", "lost", old);
  grant();

  assert.equal(await picked, "switched");
  assert.equal(await lost, "unchanged");
  assert.equal(audioIn(stream)[0].deviceId, "headset");
  assert.deepEqual(state.opened, ["headset"]);
});

// LiveKit's replace can be slow, and the interview can end inside it.
test("an interview that ends during the replace keeps nothing open", async () => {
  const opened = [];
  const { microphone, old, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  let release = null;
  sender.pending = new Promise((resolve) => {
    release = resolve;
  });

  const result = microphone.switchTo("headset");
  await new Promise((resolve) => setTimeout(resolve, 0));
  state.live = false;
  release();

  assert.equal(await result, "idle");
  assert.ok(opened[0].stopped, "the input opened for the switch stayed on");
  assert.deepEqual(audioIn(stream), [old]);
  assert.deepEqual(state.switched, []);
});

test("a refusal that arrives after the interview ended is not reported", async () => {
  let refuse = null;
  const { microphone, state } = setup({
    open: () =>
      new Promise((_, reject) => {
        refuse = () => reject(new Error("NotAllowedError"));
      }),
  });

  const result = microphone.switchTo("headset");
  await new Promise((resolve) => setTimeout(resolve, 0));
  state.live = false;
  refuse();

  assert.equal(await result, "idle");
  assert.deepEqual(state.failed, []);
});

// Unplugged while the replace was pending: its `ended` fired before anything
// listened, so committing it would leave the interview silent for good.
test("an input that ends during the replace hands back the old one", async () => {
  const opened = [];
  const { microphone, old, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  let release = null;
  sender.pending = new Promise((resolve) => {
    release = resolve;
  });

  const result = microphone.switchTo("headset");
  await new Promise((resolve) => setTimeout(resolve, 0));
  sender.pending = null;
  opened[0].readyState = "ended";
  release();

  assert.equal(await result, "refused");
  assert.equal(sender.sending, old, "the sender kept the dead input");
  assert.equal(old.stopped, false);
  assert.deepEqual(audioIn(stream), [old]);
  assert.deepEqual(state.failed, ["chosen"]);
  assert.deepEqual(state.switched, []);
});

// A device plugged in while the move after a loss is still opening queues a
// retry for the same loss; if the first attempt strands, that retry is the way
// back and must not read as already handled.
test("a retry queued behind a stranded loss still tries", async () => {
  let attempts = 0;
  let refuse = null;
  const { microphone, old, stream, state } = setup({
    open: (deviceId) => {
      attempts += 1;
      if (attempts > 1) return new FakeMic("headset");
      return new Promise((_, reject) => {
        refuse = () => reject(new Error("NotFoundError"));
      });
    },
  });
  old.readyState = "ended";

  const first = microphone.switchTo("", "lost", old);
  const retry = microphone.switchTo("", "lost", old);
  await new Promise((resolve) => setTimeout(resolve, 0));
  refuse();

  assert.equal(await first, "stranded");
  assert.equal(await retry, "switched");
  assert.equal(audioIn(stream)[0].deviceId, "headset");
  assert.deepEqual(state.outcomes, ["stranded"]);
});

// The track opened as the default reports the alias's id; picking the
// physical row of the same device is not a switch. Two real inputs of one
// interface share a group too, and moving between them is.
test("a pick in the input's own group is unchanged", async () => {
  const old = new FakeMic("default");
  old.getSettings = () => ({ deviceId: "default", groupId: "h" });
  const stream = new FakeStream([old]);
  const sender = new FakePublished(old);
  const opened = [];
  const microphone = createMicrophoneSwitch({
    open: async (deviceId) => {
      opened.push(deviceId);
      return new FakeMic(deviceId);
    },
    streamOf: () => stream,
    publishedTrack: () => sender,
    isLive: () => true,
    micEnabled: () => true,
    onSwitched: () => {},
    onFailed: () => {},
  });

  assert.equal(
    await microphone.switchTo("headset", "chosen", null, "h"),
    "unchanged",
  );
  assert.deepEqual(opened, []);
  assert.equal(
    await microphone.switchTo("laptop", "chosen", null, "l"),
    "switched",
  );
});

test("two real inputs in one group are different inputs", async () => {
  const old = new FakeMic("input-1");
  old.getSettings = () => ({ deviceId: "input-1", groupId: "interface" });
  const stream = new FakeStream([old]);
  const sender = new FakePublished(old);
  const microphone = createMicrophoneSwitch({
    open: async (deviceId) => new FakeMic(deviceId),
    streamOf: () => stream,
    publishedTrack: () => sender,
    isLive: () => true,
    micEnabled: () => true,
    onSwitched: () => {},
    onFailed: () => {},
  });

  assert.equal(
    await microphone.switchTo("input-2", "chosen", null, "interface"),
    "switched",
  );
});

// Dropping an input that still worked is the one stop the trail would not see:
// `stop()` fires no `ended`. One that had already ended recorded its own.
test("a dropped input reports whether it was still working", async () => {
  const live = setup();
  live.sender.isMuted = true;
  live.sender.refuseUnmute = true;
  await live.microphone.switchTo("headset");
  assert.deepEqual(live.state.outcomes, ["stranded"]);
  assert.deepEqual(live.state.dropped, [true]);

  const dead = setup({
    open: () => {
      throw new Error("NotFoundError");
    },
  });
  dead.old.readyState = "ended";
  await dead.microphone.switchTo("", "lost", dead.old);
  assert.deepEqual(dead.state.outcomes, ["stranded"]);
  assert.deepEqual(dead.state.dropped, [false]);
});

// LiveKit was already listening to the new input when it ended, so it muted the
// publication; handing back the old input must clear that or Jim hears nothing.
test("handing back the old input clears LiveKit's mute flag", async () => {
  const opened = [];
  const { microphone, old, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  sender.muteOnEnded = true;
  let release = null;
  sender.pending = new Promise((resolve) => {
    release = resolve;
  });

  const result = microphone.switchTo("headset");
  await new Promise((resolve) => setTimeout(resolve, 0));
  sender.pending = null;
  opened[0].readyState = "ended";
  release();

  assert.equal(await result, "refused");
  assert.equal(sender.sending, old);
  assert.equal(sender.isMuted, false, "the restored input stays muted");
  assert.deepEqual(state.outcomes, ["refused"]);
});

test("a replace that rejects after sending still hands back", async () => {
  const opened = [];
  const { microphone, old, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  sender.rejectAfterSend = true;

  assert.equal(await microphone.switchTo("headset"), "refused");
  assert.equal(sender.sending, old, "LiveKit kept sending the stopped input");
  assert.ok(opened[0].stopped);
  assert.deepEqual(audioIn(stream), [old]);
  assert.deepEqual(state.outcomes, ["refused"]);
});

// The sender is then left on the dead input, so claiming the old one is still
// in use would be false, and keeping it in the stream would make a pick of the
// same device read as unchanged.
test("a failed hand-back says so and lets the same device be picked", async () => {
  const opened = [];
  const { microphone, old, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  let release = null;
  sender.pending = new Promise((resolve) => {
    release = resolve;
  });

  const result = microphone.switchTo("headset");
  await new Promise((resolve) => setTimeout(resolve, 0));
  sender.pending = null;
  sender.refuseAfter = 1;
  opened[0].readyState = "ended";
  release();

  assert.equal(await result, "stranded");
  assert.deepEqual(state.outcomes, ["stranded"]);
  assert.ok(old.stopped);
  assert.deepEqual(audioIn(stream), []);

  sender.refuseAfter = Infinity;
  assert.equal(await microphone.switchTo("laptop"), "switched");
  assert.equal(sender.sending, audioIn(stream)[0]);
});

// The unmute after a loss is another await the interview can end inside.
test("an interview that ends during the unmute keeps nothing open", async () => {
  const opened = [];
  const { microphone, old, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  old.readyState = "ended";
  sender.isMuted = true;
  let release = null;
  sender.unmutePending = new Promise((resolve) => {
    release = resolve;
  });

  const result = microphone.switchTo("", "lost", old);
  await new Promise((resolve) => setTimeout(resolve, 0));
  state.live = false;
  release();

  assert.equal(await result, "idle");
  assert.ok(opened[0].stopped);
  assert.deepEqual(audioIn(stream), [old]);
  assert.deepEqual(state.failed, []);
});

test("a refused unmute leaves no input claimed as in use", async () => {
  const opened = [];
  const { microphone, old, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  sender.isMuted = true;
  sender.refuseUnmute = true;

  assert.equal(await microphone.switchTo("headset"), "stranded");
  // The hand-back needs the same unmute, so with it refused nothing is sent.
  assert.ok(opened[0].stopped);
  assert.ok(old.stopped, "the old input is still open but nothing sends it");
  assert.deepEqual(audioIn(stream), []);
  assert.deepEqual(state.outcomes, ["stranded"]);

  sender.refuseUnmute = false;
  sender.isMuted = false;
  assert.equal(await microphone.switchTo("laptop"), "switched");
});

test("an input that ends during the unmute is not committed", async () => {
  const opened = [];
  const { microphone, old, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  sender.isMuted = true;
  let release = null;
  sender.unmutePending = new Promise((resolve) => {
    release = resolve;
  });

  const result = microphone.switchTo("headset");
  await new Promise((resolve) => setTimeout(resolve, 0));
  sender.unmutePending = null;
  opened[0].readyState = "ended";
  release();

  assert.equal(await result, "refused");
  assert.equal(sender.sending, old);
  assert.deepEqual(audioIn(stream), [old]);
  assert.deepEqual(state.outcomes, ["refused"]);
});

// With the old input already dead there is nothing to hand back to, so the
// page must not claim one is still in use.
test("a failed move after a loss strands rather than refuses", async () => {
  const { microphone, old, stream, state } = setup({
    open: () => {
      throw new Error("NotFoundError");
    },
  });
  old.readyState = "ended";

  assert.equal(await microphone.switchTo("", "lost", old), "stranded");
  assert.deepEqual(audioIn(stream), []);
  assert.deepEqual(state.outcomes, ["stranded"]);
});

test("a new input that ends after a loss strands too", async () => {
  const opened = [];
  const { microphone, old, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  old.readyState = "ended";
  let release = null;
  sender.pending = new Promise((resolve) => {
    release = resolve;
  });

  const result = microphone.switchTo("", "lost", old);
  await new Promise((resolve) => setTimeout(resolve, 0));
  sender.pending = null;
  opened[0].readyState = "ended";
  release();

  assert.equal(await result, "stranded");
  assert.deepEqual(audioIn(stream), []);
  assert.deepEqual(state.outcomes, ["stranded"]);
});

// LiveKit mutes the publication when a user-provided track ends, so after a
// loss the interviewer would receive a muted microphone while the page reads
// "Mic on".
test("a move after a loss clears LiveKit's mute flag", async () => {
  const { microphone, old, stream, sender } = setup();
  old.readyState = "ended";
  sender.isMuted = true;

  assert.equal(await microphone.switchTo("", "lost", old), "switched");

  assert.equal(sender.isMuted, false);
  assert.equal(audioIn(stream)[0].enabled, true);
});

// The flag LiveKit raised after the loss comes down, since left up it is a
// server-side mute; a candidate who had muted themselves stays muted.
test("a muted candidate stays muted after a loss", async () => {
  const { microphone, old, stream, sender } = setup({ muted: true });
  old.readyState = "ended";
  sender.isMuted = true;

  await microphone.switchTo("", "lost", old);

  assert.equal(sender.isMuted, false);
  assert.deepEqual(sender.liveAfterReplace, [false]);
  assert.deepEqual(
    sender.liveAfterUnmute,
    [false],
    "the input went live while the flag came down",
  );
  assert.equal(audioIn(stream)[0].enabled, false);
});

test("the flag is held only for a muted candidate and lifted for anyone", async () => {
  const sender = new FakePublished(new FakeMic("laptop"));

  await holdMute(sender, true);
  assert.equal(sender.isMuted, false);
  await holdMute(sender, false);
  assert.equal(sender.isMuted, true);

  await liftMute(sender, () => false);
  assert.equal(sender.isMuted, false);
  assert.deepEqual(sender.liveAfterUnmute, [false]);

  sender.isMuted = true;
  await liftMute(sender, () => true);
  assert.deepEqual(sender.liveAfterUnmute, [false, true]);
  // Nothing to lower: no unmute, so nothing goes live.
  await liftMute(sender, () => true);
  assert.equal(sender.unmutes, 2);
  assert.deepEqual(sender.listeners.unmuted, []);
});

// The publication's flag is a server-side mute: the SFU stops forwarding, the
// agent gets no frames, and a candidate who mutes mid-sentence leaves the turn
// open. The button mutes the track only.
test("the Mic button leaves the publication's flag alone", async () => {
  const track = new FakeMic("laptop");
  const stream = new FakeStream([track]);
  stream.getAudioTracks = () => audioIn(stream);
  const sender = new FakePublished(track);
  const inFlightCalls = [];
  const scope = {
    state: { micEnabled: false, localUserStream: stream },
    nodes: { mic: { textContent: "" } },
    publishedMicrophone: () => sender,
    microphoneSwitch: () => ({
      setEnabled: (enabled) => inFlightCalls.push(enabled),
    }),
    setLocalAudioEnabled: (enabled) => {
      track.enabled = enabled;
    },
    publishIntegrityEvent: () => Promise.resolve(),
    console,
  };
  const { toggleMicrophone } = loadInterview(["toggleMicrophone"], scope);

  await toggleMicrophone();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(scope.state.micEnabled, true);
  assert.equal(track.enabled, true);
  assert.equal(scope.nodes.mic.textContent, "Mic on");

  await toggleMicrophone();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(track.enabled, false);
  assert.equal(sender.isMuted, false, "the button raised a server-side mute");
  assert.equal(sender.unmutes, 0);
  assert.deepEqual(inFlightCalls, [true, false]);
});

// Between the open and the commit the new input is in neither the stream nor
// the publication, so a mute pressed then would otherwise leave it live.
test("a mute pressed during a switch reaches the input being handed over", async () => {
  const opened = [];
  const { microphone, stream, sender, state } = setup({
    open: (deviceId) => {
      const track = new FakeMic(deviceId);
      opened.push(track);
      return track;
    },
  });
  let release = null;
  sender.pending = new Promise((resolve) => {
    release = resolve;
  });

  const result = microphone.switchTo("headset");
  await new Promise((resolve) => setTimeout(resolve, 0));
  state.muted = true;
  microphone.setEnabled(false);
  assert.equal(opened[0].enabled, false, "the input in flight stayed live");
  release();

  assert.equal(await result, "switched");
  assert.equal(audioIn(stream)[0].enabled, false);
  // Once committed it is the stream's, and the button reaches it directly.
  microphone.setEnabled(true);
  assert.equal(opened[0].enabled, false);
});

test("a throwing callback does not block the next switch", async () => {
  const old = new FakeMic("laptop");
  const stream = new FakeStream([old]);
  const sender = new FakePublished(old);
  let calls = 0;
  const microphone = createMicrophoneSwitch({
    open: async (deviceId) => new FakeMic(deviceId),
    streamOf: () => stream,
    publishedTrack: () => sender,
    isLive: () => true,
    micEnabled: () => true,
    onSwitched: () => {
      calls += 1;
      if (calls === 1) throw new Error("listener failed");
    },
    onFailed: () => {},
  });

  await assert.rejects(microphone.switchTo("headset"));
  assert.equal(await microphone.switchTo("webcam"), "switched");
});

test("the list offers named inputs only and marks the one in use", () => {
  const devices = [
    { kind: "audioinput", deviceId: "", label: "" },
    { kind: "audioinput", deviceId: "laptop", label: "Built-in Microphone" },
    { kind: "audiooutput", deviceId: "speakers", label: "Speakers" },
    { kind: "audioinput", deviceId: "usb", label: "" },
  ];

  assert.deepEqual(inputOptions(devices, { deviceId: "usb" }), [
    {
      deviceId: "laptop",
      groupId: "",
      label: "Built-in Microphone",
      selected: false,
    },
    { deviceId: "usb", groupId: "", label: "Microphone 2", selected: true },
  ]);
});

// Picking the alias of the input in use reopened it and recorded a switch that
// changed nothing, so the alias rows are not offered.
test("alias rows are left out and the default's group marks its row", () => {
  const devices = [
    { kind: "audioinput", deviceId: "default", label: "Default", groupId: "h" },
    { kind: "audioinput", deviceId: "communications", groupId: "h" },
    { kind: "audioinput", deviceId: "headset", label: "Headset", groupId: "h" },
    { kind: "audioinput", deviceId: "laptop", label: "Built-in", groupId: "l" },
  ];

  const rows = inputOptions(devices, { deviceId: "default", groupId: "h" });

  // The group travels with each row: it is how the page tells the alias in
  // use from a real switch.
  assert.deepEqual(
    rows.map((row) => [row.deviceId, row.groupId, row.selected]),
    [
      ["headset", "h", true],
      ["laptop", "l", false],
    ],
  );
});

// A track opened with no device id can report an id the list does not carry.
// Matching on the label instead keeps the select off its first row, which would
// name a microphone the candidate is not using.
test("the list falls back to the label when the id is not listed", () => {
  const devices = [
    { kind: "audioinput", deviceId: "laptop", label: "Built-in Microphone" },
    { kind: "audioinput", deviceId: "usb", label: "USB Headset" },
  ];

  const selected = (current) =>
    inputOptions(devices, current)
      .filter((option) => option.selected)
      .map((option) => option.deviceId);

  const unlisted = { deviceId: "gone", label: "USB Headset" };
  assert.deepEqual(selected(unlisted), ["usb"]);
  assert.deepEqual(selected({ ...unlisted, label: "" }), []);
});

test("the switch detail keeps a label's separators readable", () => {
  assert.equal(
    switchDetail("chosen", { label: "USB; Rear=Mic" }),
    "reason=chosen;microphone=USB; Rear=Mic",
  );
  assert.equal(
    switchDetail("lost", { label: "" }),
    "reason=lost;microphone=unnamed",
  );
});

/// The page's own listing, run against a select that records what it is given
/// and a device list the test releases when it chooses.
function listingPage({
  track = new FakeMic("laptop", "Built-in Microphone"),
} = {}) {
  const page = {
    live: true,
    pending: [],
    retried: [],
    select: {
      hidden: true,
      rows: null,
      replaceChildren(...rows) {
        this.rows = rows;
      },
    },
  };
  const stream = new FakeStream(track ? [track] : []);
  stream.getAudioTracks = () => audioIn(stream);
  const scope = {
    inputListing: 0,
    state: { localUserStream: stream },
    nodes: { micInputSelect: page.select },
    microphoneLive: () => page.live,
    inputOptions,
    navigator: {
      mediaDevices: {
        enumerateDevices: () =>
          new Promise((resolve) => page.pending.push(resolve)),
      },
    },
    document: { createElement: () => ({ dataset: {} }) },
    microphoneSwitch: () => ({
      switchTo: (...args) => {
        page.retried.push(args);
        return Promise.resolve("switched");
      },
    }),
  };
  const { refreshAudioInputs, retryMicrophone } = loadInterview(
    ["refreshAudioInputs", "retryMicrophone"],
    scope,
  );
  return { page, scope, refreshAudioInputs, retryMicrophone };
}

const twoInputs = [
  { kind: "audioinput", deviceId: "laptop", label: "Built-in Microphone" },
  { kind: "audioinput", deviceId: "usb", label: "USB Headset" },
];

// Teardown hides the select; a listing still in flight must not show it again
// over the report.
test("a listing that finishes after the interview ended paints nothing", async () => {
  const { page, refreshAudioInputs } = listingPage();

  const listing = refreshAudioInputs();
  page.live = false;
  page.pending[0](twoInputs);
  await listing;

  assert.equal(page.select.rows, null);
  assert.equal(page.select.hidden, true);
});

// Chrome adds a `default` row, and Windows a `communications` one, for the same
// physical microphone; one device is nothing to choose between.
test("one physical microphone keeps the select hidden", async () => {
  const { page, refreshAudioInputs } = listingPage();
  const oneDevice = [
    { kind: "audioinput", deviceId: "default", label: "Default", groupId: "g" },
    {
      kind: "audioinput",
      deviceId: "communications",
      label: "Comms",
      groupId: "g",
    },
    { kind: "audioinput", deviceId: "laptop", label: "Built-in", groupId: "g" },
  ];

  const listing = refreshAudioInputs();
  page.pending[0](oneDevice);
  await listing;

  assert.deepEqual(
    page.select.rows.map((row) => row.value),
    ["laptop"],
  );
  assert.equal(page.select.hidden, true);
});

test("two physical microphones show the select", async () => {
  const { page, refreshAudioInputs } = listingPage();
  const twoDevices = [
    { kind: "audioinput", deviceId: "laptop", label: "Built-in", groupId: "a" },
    { kind: "audioinput", deviceId: "usb", label: "USB Headset", groupId: "b" },
  ];

  const listing = refreshAudioInputs();
  page.pending[0](twoDevices);
  await listing;

  assert.equal(page.select.hidden, false);
});

test("an older listing that finishes last does not repaint the select", async () => {
  const { page, refreshAudioInputs } = listingPage();

  const older = refreshAudioInputs();
  const newer = refreshAudioInputs();
  page.pending[1](twoInputs);
  await newer;
  const painted = page.select.rows;
  page.pending[0]([twoInputs[0]]);
  await older;

  assert.equal(page.select.rows, painted);
  assert.equal(page.select.hidden, false);
});

// Stranded, the stream has no input. The listing still has to run, or the
// select keeps the failed pick and picking it again fires no `change`.
test("with no input the select offers a placeholder and stays visible", async () => {
  const { page, refreshAudioInputs } = listingPage({ track: null });
  const oneDevice = [
    { kind: "audioinput", deviceId: "laptop", label: "Built-in", groupId: "g" },
  ];

  const listing = refreshAudioInputs();
  page.pending[0](oneDevice);
  await listing;

  const [first, ...devices] = page.select.rows;
  assert.equal(first.disabled, true);
  assert.equal(first.selected, true);
  assert.deepEqual(
    devices.map((row) => row.value),
    ["laptop"],
  );
  assert.equal(page.select.hidden, false);
});

test("an ended input marks no row as in use", async () => {
  const dead = new FakeMic("laptop", "Built-in");
  dead.readyState = "ended";
  const { page, refreshAudioInputs } = listingPage({ track: dead });
  const oneDevice = [
    { kind: "audioinput", deviceId: "laptop", label: "Built-in", groupId: "g" },
  ];

  const listing = refreshAudioInputs();
  page.pending[0](oneDevice);
  await listing;

  assert.equal(page.select.rows[0].disabled, true);
  assert.equal(page.select.rows[1].selected, false);
  assert.equal(page.select.hidden, false);
});

// The loss that failed to move has no `ended` left to follow; a device coming
// back is what gets the candidate heard again.
test("a device change retries the move only while no input is live", () => {
  const dead = new FakeMic("laptop");
  dead.readyState = "ended";
  const stranded = listingPage({ track: null });
  const lost = listingPage({ track: dead });
  const fine = listingPage();

  stranded.retryMicrophone();
  lost.retryMicrophone();
  fine.retryMicrophone();

  assert.deepEqual(stranded.page.retried, [["", "lost", null]]);
  assert.deepEqual(lost.page.retried, [["", "lost", dead]]);
  assert.deepEqual(fine.page.retried, []);
});

test("two real inputs of one interface both get a row", async () => {
  const devices = [
    { kind: "audioinput", deviceId: "in-1", label: "Input 1", groupId: "x" },
    { kind: "audioinput", deviceId: "in-2", label: "Input 2", groupId: "x" },
  ];

  // Through the page, since counting the select's visibility by group hid
  // both rows while the rows themselves were right.
  const { page, refreshAudioInputs } = listingPage({
    track: new FakeMic("in-1", "Input 1"),
  });
  const listing = refreshAudioInputs();
  page.pending[0](devices);
  await listing;
  assert.equal(page.select.hidden, false);

  assert.deepEqual(
    inputOptions(devices, { deviceId: "in-2", groupId: "x" }).map((row) => [
      row.deviceId,
      row.selected,
    ]),
    [
      ["in-1", false],
      ["in-2", true],
    ],
  );
});

// The default alias of an interface with several inputs could be any of them,
// so its group marks no row and decides nothing.
test("an alias in a group of several inputs names none of them", () => {
  const devices = [
    { kind: "audioinput", deviceId: "default", label: "Default", groupId: "x" },
    { kind: "audioinput", deviceId: "in-1", label: "Input 1", groupId: "x" },
    { kind: "audioinput", deviceId: "in-2", label: "Input 2", groupId: "x" },
  ];

  assert.deepEqual(
    inputOptions(devices, { deviceId: "default", groupId: "x" }).filter(
      (row) => row.selected,
    ),
    [],
  );
});

test("the picked row's group is passed only when no other row shares it", () => {
  const { soleGroupOf } = loadInterview(["soleGroupOf"], {});
  const row = (value, groupId) => ({ value, dataset: { groupId } });
  const select = (picked, rows) => ({
    options: rows,
    selectedOptions: [rows.find((each) => each.value === picked)],
  });
  const rows = [row("in-1", "x"), row("in-2", "x"), row("headset", "h")];

  assert.equal(soleGroupOf(select("headset", rows)), "h");
  assert.equal(soleGroupOf(select("in-2", rows)), null);
  assert.equal(soleGroupOf(select("x", [row("x", "")])), null);
});
// The page is what records the dropped input, since the switch only reports it.
test("the page records a working input dropped by a failed switch", () => {
  let options = null;
  const published = [];
  const scope = {
    microphoneSwitchInstance: null,
    createMicrophoneSwitch: (given) => {
      options = given;
      return {};
    },
    publishIntegrityEvent: (event) => {
      published.push(event);
      return Promise.resolve();
    },
    console: { warn: () => {} },
  };
  const { microphoneSwitch } = loadInterview(["microphoneSwitch"], scope, {
    stubMissing: true,
  });
  microphoneSwitch();

  options.onFailed("chosen", new Error("x"), "stranded", true);
  options.onFailed("lost", new Error("x"), "stranded", false);
  options.onFailed("chosen", new Error("x"), "refused", false);

  assert.deepEqual(published, [
    {
      type: "MICROPHONE_STOPPED",
      source: "microphone",
      severity: "high",
      detail: "reason=switch_failed",
    },
  ]);
});

// A live input no row matches, such as a system default whose group no listed
// device shares, is still the microphone in use and is named as such.
test("a live input no row matches is named, not called missing", async () => {
  const live = new FakeMic("default", "Default Microphone");
  live.getSettings = () => ({ deviceId: "default", groupId: "virtual" });
  const { page, refreshAudioInputs } = listingPage({ track: live });
  const devices = [
    {
      kind: "audioinput",
      deviceId: "default",
      label: "Default",
      groupId: "virtual",
    },
    { kind: "audioinput", deviceId: "in-1", label: "Input 1", groupId: "a" },
    { kind: "audioinput", deviceId: "in-2", label: "Input 2", groupId: "b" },
  ];

  const listing = refreshAudioInputs();
  page.pending[0](devices);
  await listing;

  const [first] = page.select.rows;
  assert.equal(first.disabled, true);
  assert.equal(first.selected, true);
  assert.equal(first.textContent, "Default Microphone");
});
