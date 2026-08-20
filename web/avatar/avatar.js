// Jim's avatar behavior, with no Three.js and no DOM types beyond the mount's
// dataset. Everything that needs WebGL lives in ./vrm.js behind the injected
// `loadModel`, so `node --test` can drive real transitions, a real load
// failure, and a real timeout without a browser.
//
// The model handle this talks to is one method wide: `apply(pose)` once per
// frame, plus `dispose()`. A pose object beats eight setters because the whole
// frame arrives at the renderer atomically and a test can assert it by value.

// Declared here rather than in vrm.js so a caller can find out whether a model
// exists without importing the renderer to ask.
export const MODEL_URL = "/vendor/avatar/jim.vrm";

// Bounds how long a load may stay adoptable, not how long the candidate waits:
// the neutral panel is on screen for every state except `ready`, so nothing is
// staring at a spinner. That is why this is generous. An 11 MB model needs
// 8 seconds only at 11 Mbps, and at the 3 Mbps an ordinary connection actually
// delivers it needs about 30, so a shorter deadline sent every such candidate
// to the neutral panel while the download ran to completion anyway and was
// then thrown away.
export const LOAD_TIMEOUT_MS = 30000;

// Analyser. 512 bins at 48 kHz is ~10.7 ms of audio, and averaging 5 frames
// smooths the per-syllable gaps without lagging behind speech.
export const ANALYSER_FFT_SIZE = 512;
export const ANALYSER_WINDOW = 5;

// Below the threshold is room tone, not speech; above the ceiling the mouth is
// already fully open and more amplitude buys nothing.
export const MOUTH_OPEN_THRESHOLD = 0.04;
export const MOUTH_FULL_AMPLITUDE = 0.32;

export const BLINK_INTERVAL_MS = 4200;
export const BLINK_DURATION_MS = 140;
export const BREATH_PERIOD_MS = 4000;
export const BREATH_AMPLITUDE = 0.02;

// Radians. Small on purpose: an upper-body avatar that swings its head reads as
// a puppet, and the eyes carry the attention cue on their own.
export const GAZE_BOUND = 0.35;
export const HEAD_TILT_BOUND = 0.12;

export const TRANSITION_MS = 200;

// Fractions of GAZE_BOUND, so no arithmetic here can leave the bound. Thinking
// looks up and away, which is what the state means; the other two look at the
// candidate.
const GAZE_BY_STATE = {
  listening: { x: 0, y: 0 },
  thinking: { x: -1, y: 0.6 },
  speaking: { x: 0, y: 0.1 },
};

// VRM 1.0 preset expression names. Weights stay low: a permanent grin is worse
// than a neutral face.
const EXPRESSION_BY_STATE = {
  listening: { name: "relaxed", weight: 0.35 },
  thinking: { name: "neutral", weight: 0 },
  speaking: { name: "happy", weight: 0.15 },
};

// Every preset the pose can carry a weight for. The renderer writes all of
// them every frame, so an outgoing expression fades out on the same curve the
// incoming one fades in on, instead of being cut to zero.
export const EXPRESSION_NAMES = [...new Set(Object.values(EXPRESSION_BY_STATE).map((each) => each.name))];

export function clamp01(value) {
  if (!Number.isFinite(value)) return 0;
  return Math.min(Math.max(value, 0), 1);
}

/// Symmetric clamp that also maps a non-finite input to 0. Gaze and head tilt
/// go through this on the way out, not just on the way in: they were the only
/// pose fields emitted unclamped, and the contract promises every angle is
/// inside its bound.
export function clampTo(value, bound) {
  if (!Number.isFinite(value)) return 0;
  return Math.min(Math.max(value, -bound), bound);
}

export function mouthFromAmplitude(amplitude) {
  if (!Number.isFinite(amplitude) || amplitude <= MOUTH_OPEN_THRESHOLD) return 0;
  return clamp01((amplitude - MOUTH_OPEN_THRESHOLD) / (MOUTH_FULL_AMPLITUDE - MOUTH_OPEN_THRESHOLD));
}

// Deterministic in `now`, so a fake clock reproduces a blink exactly. A random
// interval would look better and could not be asserted.
export function blinkWeight(now) {
  const phase = ((now % BLINK_INTERVAL_MS) + BLINK_INTERVAL_MS) % BLINK_INTERVAL_MS;
  if (phase >= BLINK_DURATION_MS) return 0;
  const half = BLINK_DURATION_MS / 2;
  return clamp01(phase <= half ? phase / half : (BLINK_DURATION_MS - phase) / half);
}

export function breathOffset(now) {
  return Math.sin((2 * Math.PI * now) / BREATH_PERIOD_MS) * BREATH_AMPLITUDE;
}

// Scaled once at module load and frozen. This is read every frame, and the
// fractions and the bound are both constants, so there is nothing to recompute.
const GAZE_RADIANS = Object.fromEntries(
  Object.entries(GAZE_BY_STATE).map(([state, fraction]) => [
    state,
    Object.freeze({ x: fraction.x * GAZE_BOUND, y: fraction.y * GAZE_BOUND }),
  ]),
);

export function gazeForState(agentState) {
  return GAZE_RADIANS[agentState] || GAZE_RADIANS.listening;
}

export function expressionForState(agentState) {
  return EXPRESSION_BY_STATE[agentState] || EXPRESSION_BY_STATE.listening;
}

/// Exponential smoothing, frame-rate independent. The earlier `dt / durationMs`
/// form was a fixed fraction per FRAME, so the same transition took longer on a
/// 120 Hz display than on a 60 Hz one and snapped outright whenever a frame ran
/// long. `TRANSITION_MS` is a time constant, not a deadline: a change is ~63%
/// applied after it and visually finished at three times it. Nothing overshoots
/// and no clamp is needed, because the factor can never exceed 1.
function approach(current, target, dt, durationMs) {
  if (!(durationMs > 0)) return target;  // reduced motion: no easing at all
  if (!(dt > 0)) return current;         // no time has passed, so nothing moves
  return current + (target - current) * (1 - Math.exp(-dt / durationMs));
}

/// `loadModel` resolves to `{ apply(pose), dispose() }` or rejects. Rejecting,
/// timing out, and never being called all land on the same neutral panel, so a
/// browser without WebGL and a missing jim.vrm degrade identically.
export function createAvatar({
  mount,
  loadModel,
  now = () => Date.now(),
  timeoutMs = LOAD_TIMEOUT_MS,
  reducedMotion = false,
  onState = () => {},
}) {
  let model = null;
  let phase = "loading";
  let lastFrame = null;
  // Open by default, deliberately. A closed default means every path that
  // fails to report agent state produces a mute avatar, which is worse than an
  // avatar that occasionally moves its mouth when it should not.
  let mouthMuted = false;
  let agentState = "listening";
  // An explicit setGaze wins until it is cleared. Without this, setExpression
  // reassigned the gaze from the state table and silently discarded every
  // setGaze on the next agent-state change.
  let gazeOverride = null;

  // Target is where the state says to be; current is where the face is now.
  // The mouth is not in `current`, and that is the point: it is already
  // smoothed by the analyser's ANALYSER_WINDOW average before it arrives, so
  // easing it a second time lagged the jaw behind Jim's voice. Only the
  // state-driven channels, which change in steps, get eased.
  const target = { mouth: 0 };
  const current = { gazeX: 0, gazeY: 0 };
  // One weight per preset name, not one shared scalar. Sharing it meant a state
  // change swapped the expression's NAME instantly while the WEIGHT was still
  // the outgoing expression's eased value, so listening (relaxed at 0.35) to
  // speaking (happy at 0.15) applied happy at 0.35, a grin at 2.2x its target,
  // and the outgoing expression never faded at all.
  const expressionWeights = new Map(EXPRESSION_NAMES.map((name) => [name, 0]));

  // Synchronous with the dataset write, deliberately. A caller that updated the
  // panel text from the `ready` promise instead wrote it one microtask later
  // than the attribute, so anything watching the attribute could read the old
  // text, and a load that failed after destroy() still overwrote it.
  function setPhase(next) {
    phase = next;
    if (mount?.dataset) mount.dataset.avatarState = next;
    onState(next);
  }

  setPhase("loading");

  // Called inside a promise, not directly. A loadModel that throws
  // synchronously would otherwise escape past withTimeout and out of
  // createAvatar, leaving the mount stuck on "loading" forever with no panel
  // text and no rejection anyone can catch.
  const loading = Promise.resolve().then(() => loadModel());

  const ready = withTimeout(loading, timeoutMs)
    .then((loaded) => {
      if (phase === "stopped") return false;
      if (!loaded || typeof loaded.apply !== "function") throw new Error("avatar model has no apply()");
      model = loaded;
      setPhase("ready");
      return true;
    })
    .catch((error) => {
      // Swallowed for the candidate, logged for everyone else. The neutral
      // panel is the whole error report a candidate needs, and a rejection
      // here would surface as an unhandled rejection mid-interview. But every
      // failure mode collapses to the same panel by design, so without this
      // line a broken model and an absent one are indistinguishable from
      // outside the page.
      console.warn("codetrial avatar_unavailable", error);
      if (phase !== "stopped") setPhase("unavailable");
      return false;
    });

  // The timeout races the load, it cannot cancel it. A model that arrives after
  // the race was lost has already built a WebGL context and appended a canvas,
  // so without this it sits there for the rest of the interview burning one of
  // the browser's ~16 contexts, with its canvas under a neutral panel whose
  // whole claim is that no canvas exists. Chained after `ready` so it observes
  // the adoption decision rather than racing it.
  void ready
    .then(() => loading)
    .then((loaded) => {
      if (loaded && loaded !== model) loaded.dispose?.();
    })
    .catch(() => {});

  /// Amplitude drives the jaw directly. It used to be gated on `setSpeaking`
  /// having been called with true, which made the mouth depend on
  /// `lk.agent.state` arriving: when that attribute was missing or stale the
  /// gate never opened, and Jim sat through an entire interview with his mouth
  /// shut while talking. Audio is the signal that cannot lie about whether he
  /// is speaking, so it is the one in charge, and the gate defaults to open.
  function setMouth(value) {
    target.mouth = mouthMuted ? 0 : clamp01(value);
  }

  function setSpeaking(value) {
    mouthMuted = !value;
    // Zeroing here rather than waiting for the next setMouth is what makes an
    // interruption, a dropped track, or the report appearing shut the jaw on
    // the very next frame, even if no amplitude ever arrives again.
    if (mouthMuted) target.mouth = 0;
  }

  function setExpression(state) {
    agentState = state;
  }

  /// Null or undefined hands the eyes back to the agent state. Anything else
  /// pins them until it is cleared, which is why setExpression no longer
  /// touches the gaze: it used to overwrite every explicit setGaze.
  function setGaze(gaze) {
    if (gaze === null || gaze === undefined) {
      gazeOverride = null;
      return;
    }
    gazeOverride = {
      x: clampTo(gaze?.x, GAZE_BOUND),
      y: clampTo(gaze?.y, GAZE_BOUND),
    };
  }

  function gazeTarget() {
    return gazeOverride ?? gazeForState(agentState);
  }

  /// One frame. Returns the pose it pushed, or null when there is nothing to
  /// drive, so a caller can assert without reaching into the model.
  function frame(at = now()) {
    if (phase !== "ready" || !model) return null;
    // A clock that is not a finite, forward-moving number is treated as no
    // elapsed time rather than propagated. One frame(NaN) used to make gazeX
    // NaN permanently, because NaN + (target - NaN) * step stays NaN at every
    // subsequent frame, and that NaN reaches a bone rotation and makes the
    // whole skinned mesh vanish.
    const clock = Number.isFinite(at) ? at : (lastFrame ?? 0);
    // The first frame snaps to the resting pose. Easing in from a blank face at
    // startup would animate the avatar arriving, which nobody asked for; only
    // changes after that ease.
    const dt = lastFrame === null ? Infinity : Math.max(0, clock - lastFrame);
    lastFrame = clock;

    // Reduced motion keeps the speech cue and drops everything decorative:
    // no blink, no breathing, no easing between states.
    const duration = reducedMotion ? 0 : TRANSITION_MS;
    const gaze = gazeTarget();
    current.gazeX = approach(current.gazeX, gaze.x, dt, duration);
    current.gazeY = approach(current.gazeY, gaze.y, dt, duration);

    // Every preset moves every frame: the incoming one toward its own target,
    // the others toward zero. That is the cross-fade, and it is why the weight
    // can no longer be applied under the wrong name.
    const wanted = expressionForState(agentState);
    const expression = {};
    for (const name of EXPRESSION_NAMES) {
      const to = name === wanted.name ? wanted.weight : 0;
      const next = approach(expressionWeights.get(name), to, dt, duration);
      expressionWeights.set(name, next);
      expression[name] = clamp01(next);
    }

    const gazeX = clampTo(current.gazeX, GAZE_BOUND);
    const gazeY = clampTo(current.gazeY, GAZE_BOUND);
    const pose = {
      mouth: clamp01(target.mouth),
      blink: reducedMotion ? 0 : blinkWeight(clock),
      gaze: { x: gazeX, y: gazeY },
      // The head follows the eyes at a third of the travel, which is why the
      // tilt bound can never be exceeded: GAZE_BOUND/3 < HEAD_TILT_BOUND.
      headTilt: { x: gazeX / 3, y: gazeY / 3 },
      breath: reducedMotion ? 0 : breathOffset(clock),
      expression,
      agentState,
    };
    model.apply(pose);
    return pose;
  }

  function destroy() {
    setSpeaking(false);
    setPhase("stopped");
    model?.dispose?.();
    model = null;
  }

  return {
    setMouth,
    setGaze,
    setExpression,
    setSpeaking,
    frame,
    destroy,
    ready,
    state: () => phase,
    // Renders actually issued, which is not the same as frames requested: the
    // browser check needs to distinguish a live loop from an abandoned canvas.
    frames: () => model?.frames?.() ?? 0,
  };
}

function withTimeout(promise, timeoutMs) {
  if (!(timeoutMs > 0)) return Promise.resolve(promise);
  let timer = null;
  const expiry = new Promise((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error("avatar model load timed out")), timeoutMs);
  });
  return Promise.race([Promise.resolve(promise), expiry]).finally(() => clearTimeout(timer));
}
