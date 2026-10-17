import { test } from "node:test";
import assert from "node:assert/strict";
import { functionBody, read } from "./source.js";
import {
  TURN_RING_LINGER_MS,
  TURN_SPEECH_PEAK,
  TURN_WINDOW_ATTRIBUTE,
  isYieldShortcut,
  thinkingPayload,
  turnCountdown,
  turnWindowMs,
  yieldTurnPayload,
} from "../../web/lib.js";

/// Loads named functions out of web/interview.js against `scope`, which stands
/// in for the module around them. Names not in `scope` fall through to the
/// real globals, which is how `JSON` and `TextDecoder` reach them.
function loadInterview(names, scope) {
  const source = read("web/interview.js");
  const bodies = names.map((name) => `${functionBody(source, name)}\n}`);
  const exports = `{ ${names.join(", ")} }`;
  return new Function(
    "scope",
    `with (scope) { ${bodies.join("\n")}\n return ${exports}; }`,
  )(new Proxy(scope, { has: (target, key) => key in target }));
}

/// Each modifier a chord can name: the event field it sets, and the sign an
/// Apple keyboard prints on it. The page spells its chord out in one place;
/// this is what turns that spelling back into an event, so the label is
/// checked against the handler rather than against another copy of itself.
const MODIFIERS = {
  Alt: { flag: "altKey", mac: "\u2325" },
  Ctrl: { flag: "ctrlKey", mac: "\u2303" },
  Meta: { flag: "metaKey", mac: "\u2318" },
  Shift: { flag: "shiftKey", mac: "\u21e7" },
};

/// The sentence interview.html ships in #turn-status, whitespace collapsed.
function shippedTurnStatus() {
  const html = read("web/interview.html");
  const status = html.slice(html.indexOf('id="turn-status"'));
  return status
    .slice(status.indexOf(">") + 1, status.indexOf("</p>"))
    .replace(/\s+/g, " ")
    .trim();
}

test("a server thinking acknowledgement updates the controls and records the declared gap", () => {
  const state = { candidateThinking: false, endsAt: 12345, paused: false };
  const attributes = {};
  const nodes = {
    thinking: {
      textContent: "Thinking",
      setAttribute: (key, value) => {
        attributes[key] = value;
      },
    },
    turnStatus: { textContent: "" },
  };
  const events = [];
  const record = (kind, payload) => events.push({ kind, payload });
  const { applyThinking: apply } = loadInterview(["applyThinking"], {
    state,
    nodes,
    recordReplay: record,
  });
  apply(true);
  assert.equal(nodes.thinking.textContent, "Continue");
  assert.equal(attributes["aria-pressed"], "true");
  assert.match(nodes.turnStatus.textContent, /Speak again/);
  // Both strings written here land in a live region, so neither may name a
  // chord: it would be read out again on every toggle.
  assert.doesNotMatch(nodes.turnStatus.textContent, /Enter/);
  assert.equal(state.endsAt, 12345);
  assert.equal(state.paused, false);
  apply(true);
  assert.equal(events.length, 1);
  apply(false);
  assert.equal(nodes.thinking.textContent, "Thinking");
  assert.equal(attributes["aria-pressed"], "false");
  // Restoring the status writes back the line interview.html ships, not a
  // second copy of it that can drift.
  assert.equal(nodes.turnStatus.textContent, shippedTurnStatus());
  assert.doesNotMatch(nodes.turnStatus.textContent, /Enter/);
  assert.deepEqual(events, [
    { kind: "lifecycle", payload: { state: "thinking_started" } },
    { kind: "lifecycle", payload: { state: "thinking_ended" } },
  ]);
  assert.deepEqual(thinkingPayload(true), { type: "thinking", thinking: true });
  assert.deepEqual(yieldTurnPayload(), { type: "yield_turn" });
});

test("the agent's thinking acknowledgement reaches applyThinking byte for byte", () => {
  const applied = [];
  const { receiveControl } = loadInterview(["receiveControl"], {
    applyThinking: (thinking) => applied.push(thinking),
    applyPause: () => assert.fail("not a pause"),
  });
  // `thinking_state_message` in src/livekit/session.rs asserts these bytes.
  const encode = (text) => new TextEncoder().encode(text);
  receiveControl(encode('{"thinking":true,"type":"thinking_state"}'));
  receiveControl(encode('{"thinking":false,"type":"thinking_state"}'));
  receiveControl(encode('{"thinking":"yes","type":"thinking_state"}'));
  assert.deepEqual(applied, [true, false]);
});

test("the turn controls publish only in a live, connected, unpaused interview", () => {
  const sent = [];
  const state = {
    connected: true,
    interviewerPresent: true,
    paused: false,
    phase: "live",
    candidateThinking: false,
  };
  const scope = {
    state,
    nodes: { editor: {} },
    topics: { control: "control" },
    publish: (topic, payload) => {
      sent.push([topic, payload]);
      return Promise.resolve();
    },
    thinkingPayload,
    yieldTurnPayload,
    isYieldShortcut,
  };
  const { toggleThinking, yieldTurn, onTurnKey } = loadInterview(
    ["canTakeTurnAction", "toggleThinking", "yieldTurn", "onTurnKey"],
    scope,
  );
  toggleThinking();
  state.candidateThinking = true;
  toggleThinking();
  yieldTurn();
  assert.deepEqual(sent, [
    ["control", { type: "thinking", thinking: true }],
    ["control", { type: "thinking", thinking: false }],
    ["control", { type: "yield_turn" }],
  ]);

  let prevented = 0;
  const key = (target) => ({
    altKey: true,
    key: "Enter",
    target,
    preventDefault: () => {
      prevented += 1;
    },
  });
  onTurnKey(key({ tagName: "DIV" }));
  assert.equal(prevented, 1);
  assert.equal(sent.length, 4);

  for (const blocked of [
    () => (state.paused = true),
    () => (state.connected = false),
    () => (state.interviewerPresent = false),
    () => (state.phase = "ending"),
  ]) {
    Object.assign(state, {
      connected: true,
      interviewerPresent: true,
      paused: false,
      phase: "live",
    });
    blocked();
    toggleThinking();
    yieldTurn();
    onTurnKey(key({ tagName: "DIV" }));
  }
  assert.equal(sent.length, 4, "nothing published outside a live interview");
  assert.equal(prevented, 1, "the key is left alone when it does nothing");
});

test("Alt+Enter yields from the page and the code editor, never from another field", () => {
  const editor = { tagName: "TEXTAREA" };
  const press = (target, extra = {}) =>
    isYieldShortcut({ altKey: true, key: "Enter", target, ...extra }, editor);
  assert.equal(press({ tagName: "DIV" }), true);
  assert.equal(press(editor), true);
  assert.equal(press({ tagName: "INPUT" }), false);
  assert.equal(press({ tagName: "TEXTAREA" }), false);
  assert.equal(press({ tagName: "SELECT" }), false);
  assert.equal(press({ tagName: "DIV", isContentEditable: true }), false);
  assert.equal(press({ tagName: "DIV" }, { repeat: true }), false);
  assert.equal(press({ tagName: "DIV" }, { isComposing: true }), false);
  assert.equal(press({ tagName: "DIV" }, { defaultPrevented: true }), false);
  assert.equal(press({ tagName: "DIV" }, { shiftKey: true }), false);
  assert.equal(press({ tagName: "DIV" }, { ctrlKey: true }), false);
  assert.equal(press({ tagName: "DIV" }, { altKey: false }), false);
  assert.equal(press({ tagName: "DIV" }, { key: "a" }), false);
});

test("the ring fills through the published silence window and resets on speech", () => {
  const silenceMs = 3000;
  const speech = TURN_SPEECH_PEAK;
  const quiet = TURN_SPEECH_PEAK / 2;
  let spokeAt = null;
  const frame = (peak, at, blocked = false) => {
    const next = turnCountdown(spokeAt, { peak, at, silenceMs, blocked });
    spokeAt = next.spokeAt;
    return next.progress;
  };
  assert.equal(frame(quiet, 0), null, "nothing to count before speech");
  assert.equal(frame(speech, 100), 0);
  assert.equal(frame(quiet, 100 + 1500), 0.5);
  assert.equal(frame(speech, 2000), 0, "speaking again resets it");
  assert.equal(frame(quiet, 2000 + 3000), 1);
  assert.equal(frame(quiet, 2000 + 3000 + TURN_RING_LINGER_MS), 1);
  assert.equal(
    frame(quiet, 2000 + 3000 + TURN_RING_LINGER_MS + 1),
    null,
    "a full ring Jim never answered comes down",
  );
  assert.equal(frame(speech, 9000), 0);
  assert.equal(frame(quiet, 9100, true), null, "Jim speaking clears it");
  assert.equal(frame(quiet, 9200), null, "and it waits for new speech");
  assert.deepEqual(
    turnCountdown(null, { peak: speech, at: 0, silenceMs: null }),
    { spokeAt: null, progress: null },
    "no ring without a published window",
  );
});

test("the silence window is read from the agent's attribute and never guessed", () => {
  assert.equal(TURN_WINDOW_ATTRIBUTE, "codetrial.silence_ms");
  assert.equal(turnWindowMs({ [TURN_WINDOW_ATTRIBUTE]: "3000" }), 3000);
  assert.equal(turnWindowMs({ [TURN_WINDOW_ATTRIBUTE]: "0" }), null);
  assert.equal(turnWindowMs({ [TURN_WINDOW_ATTRIBUTE]: "soon" }), null);
  assert.equal(turnWindowMs({}), null);
  assert.equal(turnWindowMs(undefined), null);
});

test("the ring meters one microphone at a time and stops with its track", () => {
  const meters = [];
  const turnRing = { hidden: false, style: { setProperty() {} } };
  const scope = {
    state: { phase: "live" },
    nodes: { turnRing },
    turnSpokeAt: null,
    turnRingProgress: null,
    turnMeter: null,
    turnMeterTrack: null,
    paintTurnRing: () => {},
    startMediaMeter: () => {},
    createMicMeter: (options) => {
      const meter = {
        options,
        started: 0,
        forgotten: 0,
        start: () => (meter.started += 1),
        forget: () => (meter.forgotten += 1),
      };
      meters.push(meter);
      return meter;
    },
  };
  const { startTurnRing } = loadInterview(
    ["hideTurnRing", "stopTurnRing", "startTurnRing"],
    scope,
  );
  const track = () => {
    const listeners = {};
    return {
      readyState: "live",
      addEventListener: (name, listener) => (listeners[name] = listener),
      end: () => listeners.ended?.(),
    };
  };
  const stream = (audio) => ({ getAudioTracks: () => [audio] });

  const first = track();
  startTurnRing(stream(first));
  assert.equal(meters.length, 1);
  assert.equal(meters[0].started, 1);
  assert.equal(meters[0].options.onLevel, scope.paintTurnRing);
  startTurnRing(stream(first));
  assert.equal(meters.length, 1, "the same track keeps its meter");

  const second = track();
  startTurnRing(stream(second));
  assert.equal(meters[0].forgotten, 1, "a new track retires the old meter");
  assert.equal(meters[1].started, 1);
  first.end();
  assert.equal(meters[1].forgotten, 0, "an old track ending is ignored");
  turnRing.hidden = false;
  second.end();
  assert.equal(meters[1].forgotten, 1, "its own track ending retires it");
  assert.equal(turnRing.hidden, true);
  scope.state.phase = "ending";
  assert.equal(meters[1].options.isFinished(), true);

  startTurnRing(stream({ readyState: "ended" }));
  startTurnRing(undefined);
  assert.equal(meters.length, 2, "no meter without a live track");
});

test("the page names the yield control and its shortcut the way the handler reads them", () => {
  const html = read("web/interview.html");
  const button = html.slice(html.indexOf('id="yield-turn"'));
  const label = button.slice(
    button.indexOf(">") + 1,
    button.indexOf("</button>"),
  );
  assert.equal(label.trim(), "Your turn is done");
  const attributes = button.slice(0, button.indexOf(">"));

  // The tooltip is the one place the chord is written out in full, so it is
  // read back here and everything else is checked against what it says.
  const title = /title="([^"]+) \/ ([^"]+)"/.exec(attributes);
  assert.ok(
    title,
    `the tooltip should name a chord and its Mac spelling: ${attributes}`,
  );
  const [chord, macChord] = title.slice(1);
  const [word, key] = chord.split("+");
  const modifier = MODIFIERS[word];
  assert.ok(modifier, `the tooltip names an unknown modifier: ${chord}`);

  // Apple keyboards print that modifier as a sign, so the Mac half is the
  // same chord respelled, as an entity to keep the file ASCII.
  assert.equal(macChord, `&#${modifier.mac.codePointAt(0)};${key}`);
  assert.ok(
    attributes.includes(`aria-keyshortcuts="${chord}"`),
    `aria-keyshortcuts should name ${chord}: ${attributes}`,
  );

  // What the page prints is what the handler takes, and nothing else. A chord
  // changed in one of the two and not the other fails here, rather than
  // leaving the page naming a key that no longer does anything.
  const editor = { tagName: "TEXTAREA" };
  const press = (extra) =>
    isYieldShortcut(
      { [modifier.flag]: true, key, target: { tagName: "DIV" }, ...extra },
      editor,
    );
  assert.equal(press(), true, chord);
  for (const [other, { flag }] of Object.entries(MODIFIERS)) {
    if (other !== word) assert.equal(press({ [flag]: true }), false, other);
  }

  const ring = html.slice(html.indexOf('id="turn-ring"'));
  assert.match(
    ring.slice(0, ring.indexOf(">")),
    /aria-label="Your turn is done"/,
  );

  // #turn-status is a live region, so a chord named there is read out again on
  // every Thinking toggle, to a listener who may be on the other platform.
  // The hint carries it instead, from an element nothing rewrites.
  assert.doesNotMatch(shippedTurnStatus(), /Enter/);
  const shortcut = html.slice(html.indexOf('id="turn-shortcut"'));
  assert.doesNotMatch(
    shortcut.slice(0, shortcut.indexOf(">")),
    /role=|aria-live=/,
  );
  const hint = shortcut
    .slice(shortcut.indexOf(">") + 1, shortcut.indexOf("</p>"))
    .replace(/\s+/g, " ")
    .trim();
  // Both spellings, and only the chord is pinned: the sentence around it is
  // copy, free to be reworded without failing here.
  assert.ok(hint.includes(chord), `the hint should name ${chord}: ${hint}`);
  assert.ok(
    hint.includes(macChord),
    `the hint should name ${macChord}: ${hint}`,
  );
});
