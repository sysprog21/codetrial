// Output routing for Google Meet presentation mode.
//
// Meet re-shares the CodeTrial tab, so Jim reaches the interviewer through
// whatever speaker plays this tab. Choosing that speaker is the whole feature,
// and it is all decisions: which devices are worth offering, what to call them,
// and what to keep when the browser refuses one.
//
// Free of DOM and Web Audio types on purpose, like audio-check.js, so these
// decisions run under `node --test` instead of being asserted as source text;
// interview.js supplies the real nodes and the real `setSinkId`.

export const OUTPUT_NOTES = {
  unsupported:
    "This browser cannot choose an output device. Use the system default, or switch to Chrome.",
  listFailed: "Output devices could not be listed. The system default is in use.",
  empty:
    "No output devices yet. Finish the media preflight; this list fills in once the browser grants access.",
  refused: "That output device was refused. Jim stays on the previous device.",
};

/// Devices worth offering. Outputs only, and only those the browser has named:
/// before a `getUserMedia` grant it reports a placeholder whose `deviceId` is
/// empty, which renders as a real choice and then selects to "", so a candidate
/// picking it silently gets nothing.
export function outputOptions(devices, savedId) {
  return devices
    .filter((device) => device.kind === "audiooutput" && device.deviceId)
    .map((device, index) => ({
      deviceId: device.deviceId,
      // Labels stay blank until a permission grant; number them so the list is
      // still usable rather than a column of empty rows.
      label: device.label || `Output ${index + 1}`,
      selected: device.deviceId === savedId,
    }));
}

/// Which device the preference should name after an attempt to route.
/// `routed` is whether `setSinkId` resolved. Null means no preference at all.
///
/// The preference is written before routing is attempted, because a candidate
/// can pick a device during the preflight, long before Jim's track exists. A
/// preference that only survived a successful `setSinkId` would be dropped
/// exactly then, and the select would keep showing a device Jim never used.
/// A refusal therefore has to roll the stored value back, or the select and the
/// real sink disagree until the next reload.
export function outputAfterRouting(previous, requested, routed) {
  return routed ? requested : previous;
}
