// One output selection for preflight playback and the interviewer's voice.

import { OUTPUT_NOTES, outputOptions } from "./meet-audio.js";

let nodes = null;
let jimAudioElement = () => null;
let selectedOutputId = "";
let outputNote = "";
let listRequest = 0;
// A pre-grant listing can hide a saved device; only a granted listing can
// prove that it disappeared.
let outputAccessGranted = false;
// Serialize routes so an older setSinkId cannot finish after a newer one.
// Stamp requests to skip superseded choices and ignore their late failures.
let routingRequest = 0;
let routingChain = Promise.resolve();

export const AUDIO_OUTPUT_KEY = "codetrial:audioOutputId";
export const MEET_PRESENTATION_KEY = "codetrial:meetPresentation";

export function initAudioOutput(deps) {
  ({ nodes, jimAudioElement } = deps);
  selectedOutputId = readStored(AUDIO_OUTPUT_KEY) || "";
  renderOutputs(outputOptions([], selectedOutputId));
}

export function audioOutputId() {
  return selectedOutputId;
}

function renderOutputs(options) {
  if (
    selectedOutputId &&
    !outputAccessGranted &&
    !options.some((option) => option.deviceId === selectedOutputId)
  ) {
    options = [
      ...options,
      {
        deviceId: selectedOutputId,
        label: "Saved output device",
        selected: true,
      },
    ];
  }
  for (const select of [nodes.meetOutputSelect, nodes.preflightOutput]) {
    select.replaceChildren(
      ...options.map((option) => {
        const element = document.createElement("option");
        element.value = option.deviceId;
        element.textContent = option.label;
        element.selected = option.selected;
        return element;
      }),
    );
  }
}

function selectOutput(deviceId) {
  const changed = selectedOutputId !== deviceId;
  selectedOutputId = deviceId;
  writeStored(AUDIO_OUTPUT_KEY, deviceId);
  nodes.meetOutputSelect.value = deviceId;
  nodes.preflightOutput.value = deviceId;
  if (changed) nodes.preflightOutput.dispatchEvent(new Event("outputchange"));
}

export async function refreshAudioOutputs({ granted = false } = {}) {
  if (granted) outputAccessGranted = true;
  const request = ++listRequest;
  const mediaDevices = navigator.mediaDevices;
  if (
    !mediaDevices?.enumerateDevices ||
    !("setSinkId" in HTMLMediaElement.prototype)
  ) {
    nodes.meetOutputRow.hidden = true;
    nodes.preflightOutput.disabled = true;
    showOutputNote(OUTPUT_NOTES.unsupported);
    if (selectedOutputId) void applyAudioOutput("");
    return;
  }
  nodes.meetOutputRow.hidden = false;
  nodes.preflightOutput.disabled = false;
  try {
    const devices = await mediaDevices.enumerateDevices();
    if (request !== listRequest) return;
    const options = outputOptions(devices, selectedOutputId);
    const missing =
      outputAccessGranted &&
      !options.some((option) => option.deviceId === selectedOutputId);
    if (missing) {
      options[0].selected = true;
    }
    renderOutputs(options);
    if (outputNote !== OUTPUT_NOTES.refused)
      showOutputNote(options.length === 1 ? OUTPUT_NOTES.empty : "");
    if (missing) void applyAudioOutput("");
  } catch {
    if (request === listRequest) showOutputNote(OUTPUT_NOTES.listFailed);
  }
}

export function applyAudioOutput(deviceId) {
  deviceId = deviceId || "";
  const request = ++routingRequest;
  selectOutput(deviceId);
  routingChain = routingChain
    .catch(() => {})
    .then(() => routeAudioOutput(deviceId, request));
  return routingChain;
}

export async function audioOutputSettled() {
  let pending;
  do {
    pending = routingChain;
    await pending;
  } while (pending !== routingChain);
}

async function routeAudioOutput(deviceId, request) {
  if (request !== routingRequest) return;
  // The element changes across subscriptions; read it when routing starts.
  const jimAudio = jimAudioElement();
  if (!jimAudio) return;
  const previous = jimAudio.sinkId || "";
  try {
    if (typeof jimAudio.setSinkId === "function") {
      if (jimAudio.sinkId !== deviceId) await jimAudio.setSinkId(deviceId);
    } else if (deviceId) throw new Error("Output selection is unavailable");
    if (request === routingRequest && outputNote === OUTPUT_NOTES.refused)
      showOutputNote("");
  } catch {
    if (request !== routingRequest) return;
    // The previous sink may have disappeared while this request was pending.
    const fallback = Array.from(nodes.meetOutputSelect.options).some(
      (option) => option.value === previous,
    )
      ? previous
      : "";
    if (fallback !== previous) {
      try {
        await jimAudio.setSinkId(fallback);
      } catch {
        // Keep the refusal visible if even the system output is unavailable.
      }
    }
    if (request !== routingRequest) return;
    selectOutput(fallback);
    showOutputNote(OUTPUT_NOTES.refused);
  }
}

function showOutputNote(text) {
  outputNote = text;
  for (const note of [nodes.meetOutputNote, nodes.preflightOutputNote]) {
    note.textContent = text;
    note.hidden = !text;
  }
}

/// `localStorage` or `sessionStorage`, or `null` where site data is blocked:
/// there, touching either global throws rather than returning a storage that
/// refuses. The helpers below default through this rather than naming the
/// global, because a default parameter is evaluated before their `try`, and
/// they treat a failing call as "nothing stored", which `null` is. A caller
/// that must tell a failure from an empty store has to check for null itself.
export function storageArea(name) {
  try {
    return globalThis[name];
  } catch {
    return null;
  }
}

export function readStored(key, storage = storageArea("localStorage")) {
  try {
    return storage.getItem(key);
  } catch {
    return null;
  }
}

/// An empty value clears the key: "no preference" and "the empty preference"
/// are the same thing, and a separate clearStored existed only so one call site
/// could pick between them.
export function writeStored(key, value, storage = storageArea("localStorage")) {
  try {
    if (value) storage.setItem(key, value);
    else storage.removeItem(key);
  } catch {
    // Private browsing refuses writes; a preference is not worth failing on.
  }
}

export const JIM_VOLUME_KEY = "codetrial:jimVolume";

const sameVolume = (a, b) => Math.abs(a - b) < 0.001;

/// Some browsers ignore or reject a volume write; read it back to tell.
function setsVolume(audio, value) {
  try {
    audio.volume = value;
    return sameVolume(audio.volume, value);
  } catch {
    return false;
  }
}

/// Zero is a session mute, never a persisted level: reloading must restore the
/// last positive setting instead of starting a new interviewer silently.
export function createJimVolume({
  storage = storageArea("localStorage"),
  element = () => null,
  onChange = () => {},
} = {}) {
  const stored = readStored(JIM_VOLUME_KEY, storage);
  let level = /^(?:[1-9][0-9]?|100)$/.test(stored ?? "") ? Number(stored) : 100;
  let muted = false;
  let routingHeld = false;
  let supported = true;
  let tested = null;
  const restore = () => {
    const audio = element();
    if (
      audio &&
      (audio.muted !== (muted || routingHeld) ||
        (supported && !sameVolume(audio.volume, level / 100)))
    )
      apply();
  };
  function apply() {
    const audio = element();
    const fresh = tested !== audio;
    if (fresh) {
      tested?.removeEventListener?.("volumechange", restore);
      tested = audio;
    }
    if (audio) {
      // At 100%, a no-op setter and a working setter read identically. Probe
      // another level synchronously, then restore the requested attenuation.
      if (fresh) supported = setsVolume(audio, 0.5);
      if (supported) supported = setsVolume(audio, level / 100);
      audio.muted = muted || routingHeld;
      if (fresh) audio.addEventListener?.("volumechange", restore);
    }
    onChange();
  }
  function setVolume(value) {
    if (!Number.isInteger(value) || value < 0 || value > 100) return;
    // A muted slider sits at 0, so a single step up reads as 1%. Unmute at
    // the saved level rather than saving that step; a value the candidate
    // pointed at or jumped to directly is taken as chosen.
    if (muted && value === 1) {
      muted = false;
      apply();
      return;
    }
    muted = value === 0;
    if (value > 0) {
      level = value;
      writeStored(JIM_VOLUME_KEY, String(level), storage);
    }
    apply();
  }
  return {
    get level() {
      return level;
    },
    get muted() {
      return muted;
    },
    get supported() {
      return supported;
    },
    apply,
    setRoutingHeld(value) {
      routingHeld = Boolean(value);
      apply();
    },
    setVolume,
    toggleMute() {
      muted = !muted;
      apply();
    },
    reset: () => setVolume(100),
  };
}
