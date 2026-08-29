/// Which speaker plays the tab's audio, and the two preferences that outlive a
/// reload.
///
/// No mixer and no synthetic microphone: Meet captures the tab, and CodeTrial
/// only decides which output device the tab's own audio lands on. Split out of
/// `interview.js` because routing, the stored preference and the note the
/// candidate reads about it are one decision made in three places, and the
/// order they happen in is the part that matters.

import { OUTPUT_NOTES, outputAfterRouting, outputOptions } from "./meet-audio.js";

let nodes = null;
/// Read through a function rather than held: `playRemoteAudio` creates and
/// drops Jim's element as LiveKit re-subscribes him, and a copy taken at init
/// would route the one from the previous subscription.
let jimAudioElement = () => null;

export function initAudioOutput(deps) {
  ({ nodes, jimAudioElement } = deps);
}

export const AUDIO_OUTPUT_KEY = "codetrial:audioOutputId";
export const MEET_PRESENTATION_KEY = "codetrial:meetPresentation";

/// Lists output devices so Jim can be routed away from the shared tab's
/// default. No mixer and no synthetic microphone: Meet captures the tab, and
/// CodeTrial only decides which speaker plays the tab's own audio.
export async function refreshAudioOutputs() {
  const mediaDevices = navigator.mediaDevices;
  if (!mediaDevices?.enumerateDevices || !("setSinkId" in HTMLMediaElement.prototype)) {
    // Hide the row, not just the control: a label pointing at a hidden select
    // renders as a heading with nothing under it.
    nodes.meetOutputRow.hidden = true;
    showOutputNote(OUTPUT_NOTES.unsupported);
    return;
  }
  let devices = [];
  try {
    devices = await mediaDevices.enumerateDevices();
  } catch {
    showOutputNote(OUTPUT_NOTES.listFailed);
    return;
  }
  const options = outputOptions(devices, readStored(AUDIO_OUTPUT_KEY));
  nodes.meetOutputSelect.replaceChildren();
  for (const option of options) {
    const element = document.createElement("option");
    element.value = option.deviceId;
    element.textContent = option.label;
    element.selected = option.selected;
    nodes.meetOutputSelect.append(element);
  }
  showOutputNote(options.length ? "" : OUTPUT_NOTES.empty);
}

/// The routing request this one has to finish behind, and the number that says
/// which request is the newest.
///
/// `setSinkId` is asynchronous and the candidate can pick again before it
/// settles. Two in flight can resolve in either order, and the older one would
/// then write its device into the preference and the select, undoing a choice
/// the candidate made after it. Serialized so they resolve in the order they
/// were asked for, and stamped so a request that is no longer the newest
/// routes but does not get the last word on what is stored or shown.
let routingChain = Promise.resolve();
let routingRequest = 0;

export function applyAudioOutput(deviceId) {
  routingChain = routingChain.then(() => routeAudioOutput(deviceId, ++routingRequest));
  return routingChain;
}

async function routeAudioOutput(deviceId, request) {
  if (!deviceId) return;
  const previous = readStored(AUDIO_OUTPUT_KEY);
  writeStored(AUDIO_OUTPUT_KEY, deviceId);
  const jimAudio = jimAudioElement();
  if (!jimAudio || typeof jimAudio.setSinkId !== "function") return;
  const routed = await jimAudio
    .setSinkId(deviceId)
    .then(() => true)
    .catch(() => false);
  // Cleared, not left alone, when there is no previous device to restore. The
  // refused id was written before routing was attempted, so leaving it would
  // persist a device Jim never played through: the next reload would pre-select
  // it and retry the same refusal.
  // A newer pick arrived while this one was routing. It has already routed, so
  // Jim plays through whatever the last `setSinkId` chose, and the newest
  // request is the one entitled to say so in the preference and the select.
  if (request !== routingRequest) return;
  const stored = outputAfterRouting(previous, deviceId, routed);
  writeStored(AUDIO_OUTPUT_KEY, stored);
  nodes.meetOutputSelect.value = stored || "";
  // An empty list keeps its explanation: routing succeeding says nothing about
  // whether there was anything to choose from.
  const note = routed ? "" : OUTPUT_NOTES.refused;
  if (note || nodes.meetOutputSelect.length) showOutputNote(note);
}

export function showOutputNote(text) {
  nodes.meetOutputNote.textContent = text;
  nodes.meetOutputNote.hidden = !text;
}

export function readStored(key, storage = localStorage) {
  try {
    return storage.getItem(key);
  } catch {
    return null;
  }
}

/// An empty value clears the key: "no preference" and "the empty preference"
/// are the same thing, and a separate clearStored existed only so one call site
/// could pick between them.
export function writeStored(key, value, storage = localStorage) {
  try {
    if (value) storage.setItem(key, value);
    else storage.removeItem(key);
  } catch {
    // Private browsing refuses writes; a preference is not worth failing on.
  }
}
